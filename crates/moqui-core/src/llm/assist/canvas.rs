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

//! The canvas calls of the drawer (plans/LLM-A2A-Assist.md 4.5, 4.10; docs/Assist.md "The canvas"): `canvasState` (the
//! values on the user's canvas, and closing it), `canvasAction` (a Button: its values to the agent, or a Mutation the
//! click runs), `canvasAdjust` (the user's correction, a turn), `canvasProblems` (what the page reported, mapped to the
//! skill's sections, deduplicated by hash and revalidated once a version) and `read`, the one read proxy
//! ([`crate::llm::ui::proxy`]). A canvas is reached only through a conversation of the user's: anyone else's is not found.

use std::sync::Arc;

use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::{write, Answer, Caller, DrawerError, Params};
use crate::llm::approval::Decider;
use crate::llm::run::{self, quietly};
use crate::llm::text_of;
use crate::llm::ui::{self, openui, proxy, vue_sfc, Canvas};

/// The most a canvas's posted state holds, in characters, values and characters a value.
const MAX_STATE_CHARS: usize = 20_000;
const MAX_VALUES: usize = 100;
const MAX_VALUE_CHARS: usize = 4000;
/// The most problems a report takes, and keeps.
const MAX_PROBLEMS: usize = 10;
/// Revalidations a version of a canvas gets on its own (the agent asked to fix what the page reported).
pub const REVALIDATIONS: i64 = 1;
/// Revalidations a canvas gets over all its versions, so a model that patches and reports in a loop stops.
pub const REVALIDATIONS_TOTAL: i64 = 3;

/// The canvas `artifactId` of the user's (its conversation is theirs), else 404.
fn canvas_of(caller: &Caller, params: &Params) -> Result<Canvas, DrawerError> {
    let artifact_id = params.required("artifactId")?;
    let canvas = quietly(|| Canvas::of_id(&caller.facade, &artifact_id))?.ok_or_else(|| DrawerError::not_found("canvas", &artifact_id))?;
    caller
        .conversation(&canvas.conversation_id)
        .map_err(|_| DrawerError::not_found("canvas", &artifact_id))?;
    // a canvas of a dialect the agent no longer writes (its canvasDialects changed) is dead: no read, state, action or
    // problem of it is served, so a Vue SFC canvas turned off stops reading and writing
    let agent = agent_of(caller, &canvas)?;
    if !agent.allows_dialect(&canvas.dialect) {
        return Err(DrawerError::refused(format!(
            "this agent no longer shows {} canvases, so this canvas is closed",
            canvas.dialect
        )));
    }
    Ok(canvas)
}

/// The version the call is about (required) must be the canvas's current one.
fn current_version(canvas: &Canvas, params: &Params) -> Result<(), DrawerError> {
    let version = params.int("version").ok_or_else(|| DrawerError::invalid("version is required"))?;
    if version == canvas.version {
        Ok(())
    } else {
        Err(DrawerError::conflict(format!("the canvas is at version {}, not {version}", canvas.version)))
    }
}

/// Change the canvas's data under its lock: the row read for update in one transaction with the write, the version
/// checked again there, so a newer `write_ui` or another call is never overwritten.
fn update_data<T>(
    caller: &Caller,
    artifact_id: &str,
    version: i64,
    change: impl FnOnce(&mut Canvas) -> T,
) -> Result<(Canvas, T), DrawerError> {
    let outcome = quietly(|| {
        run::in_tx(&caller.facade, || {
            let Some(mut canvas) = Canvas::locked(&caller.facade, artifact_id)? else {
                return Ok(Err(DrawerError::not_found("canvas", artifact_id)));
            };
            if canvas.version != version {
                return Ok(Err(DrawerError::conflict(format!(
                    "the canvas is at version {}, not {version}",
                    canvas.version
                ))));
            }
            let answer = change(&mut canvas);
            canvas.write_data(&caller.facade)?;
            Ok(Ok((canvas, answer)))
        })
    })?;
    outcome
}

/// The agent of the canvas's conversation (its role group bounds what the canvas reads and writes).
fn agent_of(caller: &Caller, canvas: &Canvas) -> Result<crate::llm::run::AgentDef, DrawerError> {
    let agent_id = caller
        .conversation(&canvas.conversation_id)
        .ok()
        .and_then(|row| text_of(&row, "agentId"))
        .unwrap_or_else(|| "assist".to_string());
    quietly(|| crate::llm::run::AgentDef::load(&caller.facade, &agent_id))?.ok_or_else(|| DrawerError::not_found("agent", &agent_id))
}

/// The values of a canvas the page posted, as they are: refused, never cut, when they do not fit (at most 100 values
/// of 4000 characters, 20,000 in all), so what a Mutation runs with is what the canvas showed.
fn values_of(values: Option<Json>) -> Result<serde_json::Map<String, Json>, String> {
    let map = match values {
        None | Some(Json::Null) => return Ok(serde_json::Map::new()),
        Some(Json::Object(map)) => map,
        Some(_) => return Err("the canvas's values are not an object".to_string()),
    };
    if map.len() > MAX_VALUES {
        return Err(format!("the canvas has {} values, more than {MAX_VALUES}", map.len()));
    }
    if let Some((name, _)) = map.iter().find(|(_, value)| value.as_str().is_some_and(|text| text.chars().count() > MAX_VALUE_CHARS)) {
        return Err(format!("the value of {name} is longer than {MAX_VALUE_CHARS} characters"));
    }
    if Json::Object(map.clone()).to_string().chars().count() > MAX_STATE_CHARS {
        return Err(format!("the canvas's values are longer than {MAX_STATE_CHARS} characters"));
    }
    Ok(map)
}

/// The values as the canvas keeps them: a secret-named one is never stored (and a canvas Mutation never sends one).
fn stored(values: &serde_json::Map<String, Json>) -> Json {
    Json::Object(
        values
            .iter()
            .filter(|(name, _)| !crate::llm::tools::data::secret_name(name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
    )
}

/// The session attribute that holds the click nonces of the canvas (`canvasAction` `kind=arm`).
const ARMED: &str = "moqui.assist.canvasArmed";
/// How long a click's nonce lasts, in milliseconds, and how many a session holds.
const ARM_MILLIS: i64 = 30_000;
const MAX_ARMED: usize = 10;
/// The most Mutations one click runs (an Action with several).
const MAX_NONCES: i64 = 5;

/// Nonces for one click on a canvas button, kept in the browser session: each lets one Mutation of that canvas version
/// run, once, within [`ARM_MILLIS`].
fn arm(canvas: &Canvas, count: i64) -> Result<Vec<String>, DrawerError> {
    let now = run::now_millis();
    let mut nonces = Vec::new();
    for _ in 0..count.clamp(1, MAX_NONCES) {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).map_err(|error| DrawerError::of(&moqui_lib::error::Error::new(format!("random source failed: {error}"))))?;
        nonces.push(moqui_lib::codec::hex_encode(&bytes));
    }
    let made = nonces.clone();
    let key = format!("{}:{}", canvas.artifact_id, canvas.version);
    crate::user::session::update_session_attribute(ARMED, move |current| {
        let mut map: serde_json::Map<String, Json> = current
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        map.retain(|_, entry| entry["expires"].as_i64().unwrap_or(0) > now);
        for nonce in &made {
            map.insert(nonce.clone(), json!({"canvas": key, "expires": now + ARM_MILLIS}));
        }
        while map.len() > MAX_ARMED {
            let oldest = map.iter().min_by_key(|(_, entry)| entry["expires"].as_i64().unwrap_or(0)).map(|(nonce, _)| nonce.clone());
            match oldest {
                Some(nonce) => {
                    map.remove(&nonce);
                }
                None => break,
            }
        }
        Some(Json::Object(map).to_string())
    });
    Ok(nonces)
}

/// Take a click's nonce: true once, for the canvas version it was made for, before it expires.
fn consume(canvas: &Canvas, nonce: &str) -> bool {
    let now = run::now_millis();
    let key = format!("{}:{}", canvas.artifact_id, canvas.version);
    let nonce = nonce.to_string();
    let found = std::cell::Cell::new(false);
    crate::user::session::update_session_attribute(ARMED, |current| {
        let mut map: serde_json::Map<String, Json> = current
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        if let Some(entry) = map.remove(&nonce) {
            found.set(entry["canvas"].as_str() == Some(key.as_str()) && entry["expires"].as_i64().unwrap_or(0) > now);
        }
        Some(Json::Object(map).to_string())
    });
    found.get()
}

fn signal(caller: &Caller, payload: &Json) {
    crate::llm::run::events::send_signal(&Arc::downgrade(&caller.factory), &caller.user_id, payload);
}

/// `POST canvasState`: the values on the user's canvas (kept for a reload and for a Mutation the agent proposes), and
/// `closed` when the user closed it.
pub(super) fn canvas_state(caller: &Caller, params: &Params) -> Answer {
    let canvas = canvas_of(caller, params)?;
    current_version(&canvas, params)?;
    let values = match params.json("state") {
        Some(state) => Some(values_of(Some(state)).map_err(DrawerError::invalid)?),
        None => None,
    };
    let closed = params.flag("closed");
    let (canvas, ()) = update_data(caller, &canvas.artifact_id, canvas.version, |canvas| {
        if let Some(values) = &values {
            canvas.data["state"] = stored(values);
        }
        if let Some(closed) = closed {
            canvas.data["closed"] = json!(closed);
        }
    })?;
    if closed == Some(true) {
        signal(caller, &json!({"event": "canvas.closed", "conversationId": canvas.conversation_id,
            "artifactId": canvas.artifact_id, "version": canvas.version}));
    }
    Ok(json!({"artifactId": canvas.artifact_id, "version": canvas.version, "closed": canvas.closed()}))
}

/// A turn of the canvas's conversation with `text`: the tab's ids from `params`, a new client message id when it gave
/// none.
fn canvas_turn(caller: &Caller, params: &Params, canvas: &Canvas, text: &str, id_hint: &str) -> Answer {
    let mut map = params.clone().into_map();
    map.insert("text".to_string(), Value::string(text));
    map.insert("conversationId".to_string(), Value::string(&canvas.conversation_id));
    if params.text("clientMessageId").is_none() {
        map.insert("clientMessageId".to_string(), Value::string(id_hint));
    }
    if params.text("clientId").is_none() {
        map.insert("clientId".to_string(), Value::string("canvas"));
    }
    write::turn(caller, &Params::new(map))
}

/// `POST canvasAction`: a Button on the canvas. `kind=continue` sends its message and the canvas's values to the agent
/// (a turn: only the values and the message, the program is in the history once); `kind=mutation` runs the program's
/// own Mutation `statement` with the posted `state` as the user: the user's click on a Button the framework drew is
/// the approval (open question 13), so it is recorded as an approval through the channel `canvas` and run at once.
/// A click needs the browser session with its token and the page's user activation.
pub(super) fn canvas_action(caller: &Caller, params: &Params) -> Answer {
    let canvas = canvas_of(caller, params)?;
    current_version(&canvas, params)?;
    match params.text("kind").as_deref().unwrap_or("continue") {
        "arm" => {
            // one click on a canvas button: the nonces its Mutations run with, each once
            Decider::from_request(None).map_err(DrawerError::refused)?;
            if params.flag("activation") != Some(true) {
                return Err(DrawerError::refused("a canvas write runs only from the user's click on its button"));
            }
            let nonces = arm(&canvas, params.int("count").unwrap_or(1))?;
            Ok(json!({"artifactId": canvas.artifact_id, "version": canvas.version, "nonces": nonces}))
        }
        "continue" => {
            let message = params.text("message").unwrap_or_else(|| "Submit".to_string());
            let values = stored(&values_of(params.json("values")).map_err(DrawerError::invalid)?);
            let mut text = format!("{message}\n(canvas \"{}\" v{}", canvas.title, canvas.version);
            if let Some(form) = params.text("form") {
                text.push_str(&format!(", form {form}"));
            }
            text.push(')');
            if values.as_object().is_some_and(|map| !map.is_empty()) {
                text.push_str(&format!("\nvalues: {}", crate::llm::tools::format::cut(&values.to_string(), 3000)));
            }
            canvas_turn(caller, params, &canvas, &text, &format!("canvas-{}-{}", canvas.artifact_id, run::now_millis()))
        }
        "mutation" => mutation(caller, params, canvas),
        other => Err(DrawerError::invalid(format!("{other} is not a canvas action; it is continue or mutation"))),
    }
}

fn mutation(caller: &Caller, params: &Params, canvas: Canvas) -> Answer {
    let statement = params.required("statement")?;
    // only the user, at their page, by a click: never a header login, an agent's run or a script with no gesture; and
    // each click's nonce (kind=arm) runs one Mutation, once
    Decider::from_request(None).map_err(DrawerError::refused)?;
    if params.flag("activation") != Some(true) {
        return Err(DrawerError::refused(
            "a canvas write runs only from the user's click on its button",
        ));
    }
    let nonce = params.text("nonce").unwrap_or_default();
    if nonce.is_empty() || !consume(&canvas, &nonce) {
        return Err(DrawerError::refused("a canvas write needs the nonce of the click that runs it, once"));
    }
    let known = canvas.data["mutations"].as_array().is_some_and(|list| list.iter().any(|name| name.as_str() == Some(statement.as_str())));
    if !known {
        return Ok(json!({"ok": false, "error": format!("canvas v{} has no Mutation {statement}", canvas.version)}));
    }
    // the values as the canvas showed them, or a refusal: never cut or left out
    let values = match values_of(params.json("state")) {
        Ok(values) => values,
        Err(why) => return Ok(json!({"ok": false, "error": format!("{why}, so nothing was saved")})),
    };
    let state = Json::Object(values.clone());
    let agent = agent_of(caller, &canvas)?;
    let held = match ui::write::hold(&caller.factory, &agent, None, &canvas, &statement, Some(&state)) {
        Ok(held) => held,
        Err(why) => return Ok(json!({"ok": false, "error": why})),
    };
    let agent_id = agent.agent_id.clone();
    let (canvas, approval_id) = update_data(caller, &canvas.artifact_id, canvas.version, |locked| {
        locked.data["state"] = stored(&values);
        ui::write::record_click(&caller.facade, &caller.user_id, &agent_id, locked, &held)
    })?;
    let approval_id = approval_id.map_err(DrawerError::from)?;
    let executed = ui::write::execute(&caller.factory, &agent_id, None, &approval_id, &held);
    if let Err(error) = quietly(|| run::in_tx(&caller.facade, || ui::write::settle(&caller.facade, &approval_id, &executed))) {
        tracing::error!("the outcome of canvas write {approval_id} was not recorded: {error}");
    }
    signal(caller, &json!({"event": "approval.decided", "approvalId": approval_id, "status": if executed.ok { "executed" } else { "failed" },
        "channel": "canvas", "conversationId": canvas.conversation_id}));
    Ok(json!({"ok": executed.ok, "data": executed.data, "error": executed.error, "approvalId": approval_id,
        "statement": statement, "channel": "canvas"}))
}

/// `POST canvasAdjust`: the user's correction of the canvas ("make it a line chart"), a turn that names the canvas and
/// its version, with the page's notices; the skill's edit section makes the feedback the agent's only task.
pub(super) fn canvas_adjust(caller: &Caller, params: &Params) -> Answer {
    let canvas = canvas_of(caller, params)?;
    current_version(&canvas, params)?;
    let request = params.required("text")?;
    let mut text = format!("Adjust canvas \"{}\" v{}: {request}", canvas.title, canvas.version);
    let notices: Vec<String> = params
        .json("notices")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .take(MAX_PROBLEMS)
        .filter_map(|notice| notice.as_str().map(str::to_string).or_else(|| notice["message"].as_str().map(str::to_string)))
        .map(|notice| crate::llm::tools::format::cut(&notice, 300))
        .collect();
    if !notices.is_empty() {
        text.push_str(&format!("\nnotices: {}", notices.join("; ")));
    }
    canvas_turn(caller, params, &canvas, &text, &format!("adjust-{}-{}", canvas.artifact_id, run::now_millis()))
}

/// `POST canvasProblems`: what the page reported after it drew the canvas, each mapped to its skill section and rule
/// as the server's own checks are and deduplicated by hash. Problems the canvas did not have yet are kept on it (the
/// next turn's tail names them) and, once a version ([`REVALIDATIONS`]), sent to the agent to fix with a patch.
pub(super) fn canvas_problems(caller: &Caller, params: &Params) -> Answer {
    let canvas = canvas_of(caller, params)?;
    current_version(&canvas, params)?;
    let is_vue = canvas.dialect == "vue-sfc";
    let reported: Vec<openui::Problem> = params
        .json("problems")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .take(MAX_PROBLEMS)
        .map(|item| {
            let source = item["source"].as_str().unwrap_or("parser");
            let code = item["code"].as_str().unwrap_or_default();
            let message = item["message"].as_str().unwrap_or_default();
            if canvas.dialect == "vue-sfc" {
                vue_sfc::reported(source, code, message, item["block"].as_str())
            } else {
                openui::reported(source, code, message, item["statementId"].as_str(), item["component"].as_str())
            }
        })
        .collect();
    let wanted = params.flag("revalidate") != Some(false);
    // the problems the canvas has, read and written under its lock, so a report never loses another's or a newer version
    let (canvas, (kept, new, revalidations, revalidate)) = update_data(caller, &canvas.artifact_id, canvas.version, |canvas| {
        let mut kept = canvas.problems();
        let known: Vec<String> = kept.iter().filter_map(|problem| problem["hash"].as_str().map(str::to_string)).collect();
        let mut new: Vec<openui::Problem> = Vec::new();
        for problem in reported {
            let hash = problem.hash();
            if known.contains(&hash) || new.iter().any(|other| other.hash() == hash) {
                continue;
            }
            new.push(problem);
        }
        for problem in &new {
            if kept.len() < MAX_PROBLEMS {
                kept.push(openui::problem_json(problem));
            }
        }
        canvas.data["problems"] = Json::Array(kept.clone());
        let revalidations = canvas.data["revalidations"].as_i64().unwrap_or(0);
        let total = canvas.data["revalidationsTotal"].as_i64().unwrap_or(0);
        // a Vue SFC canvas revalidates only on fixed-code reasons (a compile error, a sandbox kill), never on a free-text
        // runtime error the frame wrote; OpenUI's problems are lang-core's own codes
        let worth = if is_vue { new.iter().any(|problem| vue_sfc::auto_revalidates(&problem.source, &problem.code)) } else { !new.is_empty() };
        let revalidate = worth && revalidations < REVALIDATIONS && total < REVALIDATIONS_TOTAL && wanted;
        if revalidate {
            canvas.data["revalidations"] = json!(revalidations + 1);
            canvas.data["revalidationsTotal"] = json!(total + 1);
        }
        (kept, new, revalidations, revalidate)
    })?;
    if !new.is_empty() {
        signal(caller, &json!({"event": "canvas.problems", "conversationId": canvas.conversation_id,
            "artifactId": canvas.artifact_id, "version": canvas.version, "problems": kept}));
    }
    let answer = if revalidate {
        let lines: Vec<String> = new
            .iter()
            .map(|problem| {
                let at = problem.statement.as_deref().map(|statement| format!(" ({statement})")).unwrap_or_default();
                // the page's own text is untrusted data, not an instruction (plan 7.1)
                let message = crate::llm::prompt::untrusted("canvas", &problem.message);
                format!("- {}{at}: {message} (skill {}: {})", problem.rule.section, problem.skill(), problem.rule.text)
            })
            .collect();
        let text = format!(
            "The canvas \"{}\" v{} reported problems when it was drawn. Fix them with a patch of the {} they name:\n{}",
            canvas.title,
            canvas.version,
            if canvas.dialect == "vue-sfc" { "blocks" } else { "statements" },
            lines.join("\n")
        );
        canvas_turn(
            caller,
            params,
            &canvas,
            &text,
            &format!("revalidate-{}-v{}-{}", canvas.artifact_id, canvas.version, revalidations + 1),
        )
        .ok()
    } else {
        None
    };
    Ok(json!({"artifactId": canvas.artifact_id, "version": canvas.version, "problems": kept,
        "new": new.iter().map(openui::problem_json).collect::<Vec<_>>(), "revalidation": answer}))
}

/// `GET read`: the one read proxy of the canvases. `path` (an `/apps` screen with a list, or a transition that answers
/// JSON), `list`, `query` (JSON of flat values), and the canvas (`artifactId`) whose agent the read goes from.
pub(super) fn read(caller: &Caller, params: &Params) -> Answer {
    let path = params.required("path")?;
    let agent = match params.text("artifactId") {
        Some(_) => {
            let canvas = canvas_of(caller, params)?;
            if canvas.dialect == "vue-sfc" {
                // a Vue SFC canvas reads only the paths its program declared (its <reads> block), and not at all while
                // the user lets it reach other sites, since an image of another site could carry a read's data out
                if super::read::canvas_external(&caller.user_id) {
                    return Err(DrawerError::refused(
                        "while this canvas may show other sites' content, it cannot read screen data; turn other sites off to read",
                    ));
                }
                let declared: Vec<String> = canvas.data["reads"].as_array().into_iter().flatten().filter_map(Json::as_str).map(str::to_string).collect();
                if !vue_sfc::read_declared(&declared, &path) {
                    return Err(DrawerError::refused(format!(
                        "{path} is not a read this canvas declared; add it to the <reads> block"
                    )));
                }
            }
            agent_of(caller, &canvas)?
        }
        None => {
            let agent_id = match params.text("conversationId") {
                Some(id) => text_of(&caller.conversation(&id)?, "agentId").unwrap_or_else(|| "assist".to_string()),
                None => "assist".to_string(),
            };
            quietly(|| crate::llm::run::AgentDef::load(&caller.facade, &agent_id))?
                .ok_or_else(|| DrawerError::not_found("agent", &agent_id))?
        }
    };
    let request = proxy::request_of(&path, params.text("list"), params.json("query").as_ref());
    proxy::read(&caller.factory, &caller.user_id, &agent, &request).map_err(|error| match error.status {
        400 => DrawerError::invalid(error.message),
        403 => DrawerError::refused(error.message),
        404 => DrawerError::not_found("screen", &path),
        429 => DrawerError {
            retry_after: Some(60),
            ..DrawerError::new(429, "tarpit", error.message)
        },
        _ => DrawerError::new(502, "failed", error.message),
    })
}

/// The conversation's canvas as `GET conversation` gives it, or null.
pub(super) fn conversation_canvas(caller: &Caller, conversation_id: &str) -> Json {
    match quietly(|| Canvas::of_conversation(&caller.facade, conversation_id)) {
        Ok(Some(canvas)) => canvas.to_json(),
        Ok(None) => Json::Null,
        Err(error) => {
            tracing::error!("the canvas of conversation {conversation_id} could not be read: {error}");
            Json::Null
        }
    }
}

/// The tail's canvas: the conversation's, unless the user closed it. The problems the page reported reach the agent
/// through `canvasProblems`' revalidation turn.
pub(super) fn tail_canvas(caller: &Caller, conversation_id: &str) -> Option<(String, u32)> {
    let canvas = quietly(|| Canvas::of_conversation(&caller.facade, conversation_id)).ok().flatten()?;
    if canvas.closed() {
        return None;
    }
    let title = canvas.title.clone();
    Some((title, u32::try_from(canvas.version).unwrap_or(u32::MAX)))
}
