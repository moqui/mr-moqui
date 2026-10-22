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

//! A2A push notifications (plan 4.15): the configs of a task (`AgentPushConfig`, the token and the credentials
//! encrypted and never returned) and their delivery from an outbox (`AgentPushDelivery`).
//!
//! As the A2A 1.0 schema (`TaskPushNotificationConfig`, `AuthenticationInfo`) and its reference sender have it, a
//! notification is a POST of a `StreamResponse` with the config's `token` in `X-A2A-Notification-Token` and its
//! `authentication` as `Authorization: <scheme> <credentials>`; the 1.0 schema has no signed notifications, so none
//! are signed. A push URL is absolute http(s) with no user info or fragment, https unless `push allow-http`, and never
//! an internal address: the client refuses one written out and one a name resolves to, and follows no redirect. The
//! conf's `push allow-hosts` lifts both checks for the hosts it names (a loopback receiver in a test).
//!
//! A thread per node ([`start`], while the server is on) does one round a second ([`tick`]): the new `run.state`
//! events of the tasks it follows become outbox rows (each state a `statusUpdate`; a completed task's whole answer
//! first as an `artifactUpdate`), numbered by their event, made once under the config's row lock; then the head row of
//! each config that has one due is sent, claimed with a conditional update so one node sends it, by a few threads
//! with at most two posts at once to one host, a short connect timeout and a time budget per round, so a dead
//! receiver holds up only its own configs. A failure waits 1 min, 5 min, 30 min, then 2 h, up to `max-attempts` in
//! all; a row that never got through is failed and the next one goes. A task takes at most 10 configs and a caller 50;
//! a config lasts 7 days, after which the thread removes it with its notifications (and delivered or failed
//! notifications of that age), once an hour.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::http::RestClient;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use serde_json::{json, Value as Json};

use super::ops::{self, Ctx, Follow, Projector, TaskRow};
use super::types::{self, A2aError, PushIn};
use super::{Failure, Origin};
use crate::context::ExecutionContextFactory;
use crate::entity::{EntityFacade, EntityValue};
use crate::llm::run::{self, in_tx, now_millis, quietly, stamp};
use crate::llm::{int_of, millis_of, text_of};

const CONFIG: &str = "moqui.agent.AgentPushConfig";
const DELIVERY: &str = "moqui.agent.AgentPushDelivery";
const RUN_EVENT: &str = "moqui.agent.AgentRunEvent";
/// Configs a task may have, and a user in all.
const MAX_CONFIGS: i64 = 10;
const MAX_USER_CONFIGS: i64 = 50;
/// The waits after each failed attempt.
const BACKOFF: [i64; 4] = [60_000, 300_000, 1_800_000, 7_200_000];
/// A config lasts this long: it is followed until then and removed after ([`clean`]), with its notifications.
const CONFIG_DAYS: i64 = 7;
const DAY: i64 = 86_400_000;
/// How often the push thread cleans up.
const CLEAN_EVERY: i64 = 3_600_000;
/// A response body read of a receiver, at most.
const RESPONSE_CAP: u64 = 16 * 1024;
/// How long a receiver's connection may take to open.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// The threads that send, at most this many to one host at once, and the time a round may take.
const WORKERS: usize = 4;
const PER_HOST: usize = 2;
const ROUND_BUDGET: Duration = Duration::from_secs(20);
/// Configs a round sends for, and the events of a config it projects.
const MAX_CONFIGS_PER_ROUND: usize = 200;
const PROJECT_PAGE: i64 = 50;

/// The host of an absolute http(s) URL in lower case, and its scheme; `None` for anything else, a URL with user info
/// or a fragment.
pub(crate) fn scheme_host(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    if url.chars().any(|c| c <= ' ' || c == '\u{7f}') || url.contains('#') {
        return None;
    }
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or_default().to_string()
    } else {
        authority
            .rsplit_once(':')
            .map_or(authority, |(host, _)| host)
            .to_string()
    };
    (!host.is_empty()).then(|| (scheme, host.to_ascii_lowercase()))
}

/// Whether the conf names `host` in `push allow-hosts`.
fn allowed_host(factory: &ExecutionContextFactory, host: &str) -> bool {
    crate::llm::facade(factory)
        .conf()
        .a2a
        .push
        .allow_hosts
        .iter()
        .any(|allowed| allowed.trim().eq_ignore_ascii_case(host))
}

/// Check a push URL before it is kept: absolute http(s) without user info or fragment, https unless allowed, and not
/// an internal address written out (a name is checked when it is resolved for the post).
///
/// # Errors
///
/// InvalidParams naming what does not fit.
pub(crate) fn check_url(factory: &ExecutionContextFactory, url: &str) -> Result<(), A2aError> {
    let Some((scheme, host)) = scheme_host(url) else {
        return Err(A2aError::invalid_params(
            "the push URL must be an absolute http or https URL with no user info or fragment",
        ));
    };
    if url.len() > 2000 {
        return Err(A2aError::invalid_params("the push URL is longer than 2000 characters"));
    }
    let llm = crate::llm::facade(factory);
    let conf = &llm.conf().a2a.push;
    let allowed = allowed_host(factory, &host);
    if scheme == "http" && !conf.allow_http && !allowed {
        return Err(A2aError::invalid_params("the push URL must be https"));
    }
    if !allowed {
        let internal = host == "localhost"
            || host.ends_with(".localhost")
            || moqui_lib::http::host_address(url).is_some_and(moqui_lib::http::internal_address);
        if internal {
            return Err(A2aError::invalid_params("the push URL must be a public address"));
        }
    }
    Ok(())
}

/// A config's wire object: never the token or the credentials.
fn config_json(row: &EntityValue) -> Json {
    let mut config =
        json!({"id": text_of(row, "pushConfigId"), "taskId": text_of(row, "runId"), "url": text_of(row, "url")});
    if let Some(scheme) = settings(row).0.map(|(scheme, _)| scheme) {
        config["authentication"] = json!({"scheme": scheme});
    }
    config
}

/// A config's authentication and the base URL its links are made with (`authenticationJson`, encrypted).
fn settings(row: &EntityValue) -> (Option<(String, Option<String>)>, Option<String>) {
    let stored = text_of(row, "authenticationJson")
        .and_then(|text| serde_json::from_str::<Json>(&text).ok())
        .unwrap_or(Json::Null);
    let authentication = stored.get("scheme").and_then(Json::as_str).map(|scheme| {
        (
            scheme.to_string(),
            stored.get("credentials").and_then(Json::as_str).map(str::to_string),
        )
    });
    (
        authentication,
        stored.get("baseUrl").and_then(Json::as_str).map(str::to_string),
    )
}

fn new_id() -> Result<String, Error> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| Error::new(format!("random source failed: {error}")))?;
    Ok(moqui_lib::codec::hex_encode(&bytes))
}

/// Keep a config for `task` (the URL was checked): the client's id when it is a plain one that is free or already this
/// task's (which is then replaced), else a new one.
///
/// # Errors
///
/// InvalidParams for a bad URL or too many configs.
pub(crate) fn create(ctx: &Ctx<'_>, task: &TaskRow, push: &PushIn) -> Result<Json, Failure> {
    check_url(ctx.factory, &push.url)?;
    let facade = &ctx.facade;
    let failed = |error: Error| Failure::of(&error, "a push notification config");
    let stored = quietly(|| -> Result<Result<Json, A2aError>, Error> {
        in_tx(facade, || {
            let existing = match push.id.as_deref().filter(|id| types::plain_id(id)) {
                Some(id) => facade
                    .find(CONFIG)?
                    .eq("pushConfigId", Value::string(id))
                    .for_update(true)
                    .disable_authz()
                    .one()?,
                None => None,
            };
            let mine = existing.as_ref().is_some_and(|row| {
                text_of(row, "runId").as_deref() == Some(task.id())
                    && text_of(row, "userId").as_deref() == Some(ctx.caller.user_id.as_str())
            });
            let mut row = match (existing, mine) {
                (Some(row), true) => row,
                (taken, _) => {
                    let count = facade
                        .find(CONFIG)?
                        .eq("runId", Value::string(task.id()))
                        .disable_authz()
                        .count()?;
                    if count >= MAX_CONFIGS {
                        return Ok(Err(A2aError::invalid_params(format!(
                            "a task takes at most {MAX_CONFIGS} push notification configs"
                        ))));
                    }
                    let of_user = facade
                        .find(CONFIG)?
                        .eq("userId", Value::string(&ctx.caller.user_id))
                        .disable_authz()
                        .count()?;
                    if of_user >= MAX_USER_CONFIGS {
                        return Ok(Err(A2aError::invalid_params(format!(
                            "a caller has at most {MAX_USER_CONFIGS} push notification configs; delete some first"
                        ))));
                    }
                    let id = match push.id.as_deref().filter(|id| types::plain_id(id) && taken.is_none()) {
                        Some(id) => id.to_string(),
                        None => new_id()?,
                    };
                    let mut row = facade.make_value(CONFIG)?;
                    row.set("pushConfigId", Value::string(&id));
                    row.set("runId", Value::string(task.id()));
                    row.set("userId", Value::string(&ctx.caller.user_id));
                    row.set("createdDate", stamp(now_millis()));
                    row.create(facade)?;
                    row
                }
            };
            row.set("url", Value::string(&push.url));
            row.set("token", push.token.as_ref().map_or(Value::Null, Value::string));
            let mut settings = json!({"baseUrl": ctx.caller.base_url});
            if let Some((scheme, credentials)) = &push.authentication {
                settings["scheme"] = json!(scheme);
                settings["credentials"] = json!(credentials);
            }
            row.set("authenticationJson", Value::string(settings.to_string()));
            row.update(facade)?;
            Ok(Ok(config_json(&row)))
        })
    })
    .map_err(failed)?;
    stored.map_err(Failure::A2a)
}

/// `CreateTaskPushNotificationConfig`.
///
/// # Errors
///
/// TaskNotFound, InvalidParams.
pub(crate) fn create_op(ctx: &Ctx<'_>, params: &Json) -> Result<Json, Failure> {
    let push = types::parse_push(params)?;
    let task_id = push
        .task_id
        .clone()
        .ok_or_else(|| A2aError::invalid_params("taskId is required"))?;
    let task = ops::owned(ctx, &task_id)?;
    create(ctx, &task, &push)
}

fn config_of(ctx: &Ctx<'_>, task_id: &str, config_id: &str) -> Result<EntityValue, Failure> {
    ops::owned(ctx, task_id)?;
    quietly(|| {
        ctx.facade
            .find(CONFIG)?
            .eq("pushConfigId", Value::string(config_id))
            .eq("runId", Value::string(task_id))
            .eq("userId", Value::string(&ctx.caller.user_id))
            .disable_authz()
            .one()
    })
    .map_err(|error| Failure::of(&error, "a push notification config"))?
    .ok_or_else(|| {
        Failure::A2a(A2aError::new(
            -32001,
            format!("there is no push notification config {config_id} of task {task_id}"),
        ))
    })
}

/// `GetTaskPushNotificationConfig`.
///
/// # Errors
///
/// TaskNotFound for a task or config that is not the caller's.
pub(crate) fn get_op(ctx: &Ctx<'_>, params: &Json) -> Result<Json, Failure> {
    let task_id = ops::required_id(params, "taskId")?;
    let config_id = ops::required_id(params, "id")?;
    Ok(config_json(&config_of(ctx, &task_id, &config_id)?))
}

/// `ListTaskPushNotificationConfigs`: `pageSize` (1 to 100, 50) at a time from `pageToken`.
///
/// # Errors
///
/// TaskNotFound, InvalidParams.
pub(crate) fn list_op(ctx: &Ctx<'_>, params: &Json) -> Result<Json, Failure> {
    let task_id = ops::required_id(params, "taskId")?;
    let size = types::page_size(params.get("pageSize"))?;
    let offset = match params
        .get("pageToken")
        .and_then(Json::as_str)
        .filter(|token| !token.is_empty())
    {
        Some(token) => base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(token)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|text| text.strip_prefix("o:").and_then(|n| n.parse::<i64>().ok()))
            .filter(|offset| *offset >= 0)
            .ok_or_else(|| A2aError::invalid_params("pageToken is not valid"))?,
        None => 0,
    };
    ops::owned(ctx, &task_id)?;
    let rows = quietly(|| {
        ctx.facade
            .find(CONFIG)?
            .eq("runId", Value::string(&task_id))
            .eq("userId", Value::string(&ctx.caller.user_id))
            .order_by("createdDate", false)
            .order_by("pushConfigId", false)
            .offset(offset)
            .limit(i64::try_from(size + 1).unwrap_or(101))
            .disable_authz()
            .list()
    })
    .map_err(|error| Failure::of(&error, "ListTaskPushNotificationConfigs"))?;
    let rows = rows.values();
    let more = rows.len() > size;
    let configs: Vec<Json> = rows.iter().take(size).map(config_json).collect();
    let next = if more {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(format!("o:{}", offset + i64::try_from(size).unwrap_or(0)))
    } else {
        String::new()
    };
    Ok(json!({"configs": configs, "nextPageToken": next}))
}

/// `DeleteTaskPushNotificationConfig`: the config and what it had still to deliver.
///
/// # Errors
///
/// TaskNotFound for a task or config that is not the caller's.
pub(crate) fn delete_op(ctx: &Ctx<'_>, params: &Json) -> Result<Json, Failure> {
    let task_id = ops::required_id(params, "taskId")?;
    let config_id = ops::required_id(params, "id")?;
    let row = config_of(ctx, &task_id, &config_id)?;
    quietly(|| {
        in_tx(&ctx.facade, || {
            ctx.facade
                .find(DELIVERY)?
                .eq("pushConfigId", Value::string(&config_id))
                .disable_authz()
                .delete_all()?;
            row.delete(&ctx.facade)
        })
    })
    .map_err(|error| Failure::of(&error, "DeleteTaskPushNotificationConfig"))?;
    Ok(json!({}))
}

// ---------- delivery ----------

/// A config the push thread follows: its task, the last event it projected, when it was made and its base URL.
struct Tracked {
    run_id: String,
    cursor: i64,
    created: i64,
}

/// What a node's push thread keeps between rounds: the configs it follows, the newest config time it has read, and
/// when it last cleaned up.
#[derive(Default)]
struct PushState {
    tracked: HashMap<String, Tracked>,
    watermark: Option<i64>,
    cleaned: Option<i64>,
}

/// This factory's push thread and its state.
#[derive(Default)]
struct Pusher {
    started: AtomicBool,
    state: Mutex<PushState>,
}

/// Start the push thread of this node, once, while the A2A server is on: a [`tick`] every second until the factory
/// goes. A round that fails is logged and the next one tries again.
pub fn start(factory: &Arc<ExecutionContextFactory>) {
    if !super::enabled(factory) || factory.entity().is_none() {
        return;
    }
    if factory
        .extension(|_| Pusher::default())
        .started
        .swap(true, Ordering::SeqCst)
    {
        return;
    }
    let weak = Arc::downgrade(factory);
    let spawned = std::thread::Builder::new()
        .name("moqui-a2a-push".to_string())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            let Some(factory) = weak.upgrade() else {
                return;
            };
            let context = factory.new_context();
            let _entered = context.enter();
            let _authz = context.disable_authz();
            if let Err(error) = tick(&factory) {
                tracing::error!("the A2A push round failed: {error}");
            }
        });
    if let Err(error) = spawned {
        tracing::error!("the A2A push thread did not start: {error}");
    }
}

/// One round: new outbox rows from the followed tasks' new events, once an hour the cleanup, then the deliveries that
/// are due. Out: the notifications delivered. Each part that fails is logged and the next one goes on, as does every
/// config and delivery within a part.
///
/// # Errors
///
/// Returns an error when the entity facade is not installed or the outbox cannot be read.
pub fn tick(factory: &Arc<ExecutionContextFactory>) -> Result<usize, Error> {
    let facade = run::facade_of(factory)?;
    let pusher = factory.extension(|_| Pusher::default());
    {
        let mut state = lock(&pusher.state);
        if let Err(error) = quietly(|| project(factory, &facade, &mut state)) {
            tracing::error!("the A2A push notifications could not be made this round: {error}");
        }
        let now = now_millis();
        if state.cleaned.is_none_or(|at| now - at >= CLEAN_EVERY) {
            state.cleaned = Some(now);
            match quietly(|| clean(&facade, now)) {
                Ok((configs, deliveries)) if configs + deliveries > 0 => {
                    tracing::info!("removed {configs} old A2A push configs and {deliveries} push notifications");
                }
                Ok(_) => {}
                Err(error) => tracing::error!("the old A2A push rows could not be removed: {error}"),
            }
        }
    }
    let deadline = Instant::now() + ROUND_BUDGET;
    quietly(|| deliver(factory, &facade, deadline))
}

/// Make the outbox rows of the followed configs whose task has events past what was projected. A config is read once,
/// when it is new to this node (each round reads only the configs made since the last); it is let go once its task
/// ended and every event of it was projected, when it is gone, or after [`CONFIG_DAYS`].
fn project(factory: &Arc<ExecutionContextFactory>, facade: &EntityFacade, state: &mut PushState) -> Result<(), Error> {
    let now = now_millis();
    let since = state.watermark.unwrap_or(now - CONFIG_DAYS * DAY) - 5_000;
    let new_rows = facade
        .find(CONFIG)?
        .condition(Condition::greater_equal("createdDate", stamp(since)))
        .select_field("pushConfigId")
        .select_field("runId")
        .select_field("createdDate")
        .disable_authz()
        .list()?;
    state.watermark = Some(now);
    let new_ids: Vec<String> = new_rows
        .values()
        .iter()
        .filter_map(|row| text_of(row, "pushConfigId"))
        .filter(|id| !state.tracked.contains_key(id))
        .collect();
    if !new_ids.is_empty() {
        // where another node, or this one before a restart, got to
        let mut cursors: HashMap<String, i64> = HashMap::new();
        for row in facade
            .find(DELIVERY)?
            .condition(Condition::is_in(
                "pushConfigId",
                new_ids.iter().map(Value::string).collect(),
            ))
            .select_field("pushConfigId")
            .select_field("eventSeq")
            .disable_authz()
            .list()?
            .values()
        {
            let cursor = cursors
                .entry(text_of(row, "pushConfigId").unwrap_or_default())
                .or_insert(0);
            *cursor = (*cursor).max(int_of(row, "eventSeq").unwrap_or(0));
        }
        for row in new_rows.values() {
            let Some(id) = text_of(row, "pushConfigId").filter(|id| new_ids.contains(id)) else {
                continue;
            };
            state.tracked.insert(
                id.clone(),
                Tracked {
                    run_id: text_of(row, "runId").unwrap_or_default(),
                    cursor: cursors.get(&id).copied().unwrap_or(0),
                    created: row.get("createdDate").and_then(millis_of).unwrap_or(now),
                },
            );
        }
    }
    if state.tracked.is_empty() {
        return Ok(());
    }
    let mut run_ids: Vec<Value> = state
        .tracked
        .values()
        .map(|tracked| Value::string(&tracked.run_id))
        .collect();
    run_ids.dedup();
    let tasks: HashMap<String, TaskRow> = facade
        .find(super::ops::RUN)?
        .condition(Condition::is_in("runId", run_ids))
        .disable_authz()
        .list()?
        .values()
        .iter()
        .map(TaskRow::of)
        .map(|task| (task.id().to_string(), task))
        .collect();
    let mut done = Vec::new();
    for (config_id, tracked) in &mut state.tracked {
        let Some(task) = tasks.get(&tracked.run_id) else {
            done.push(config_id.clone());
            continue;
        };
        if tracked.created < now - CONFIG_DAYS * DAY {
            done.push(config_id.clone());
            continue;
        }
        if task.last_event_seq > tracked.cursor {
            match project_config(factory, facade, config_id, task, tracked.cursor) {
                Ok(Some(cursor)) => tracked.cursor = cursor,
                Ok(None) => {
                    done.push(config_id.clone());
                    continue;
                }
                Err(error) => {
                    tracing::error!(
                        "the push notifications of config {config_id} (task {}) could not be made: {error}",
                        task.id()
                    );
                    continue;
                }
            }
        }
        if task.run.state.is_terminal() && tracked.cursor >= task.last_event_seq {
            done.push(config_id.clone());
        }
    }
    for config_id in done {
        state.tracked.remove(&config_id);
    }
    Ok(())
}

/// The outbox rows of a config's `run.state` events after `cursor`. Out: the new cursor, or `None` when the config is
/// gone.
fn project_config(
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    config_id: &str,
    task: &TaskRow,
    cursor: i64,
) -> Result<Option<i64>, Error> {
    let Some(config) = facade
        .find(CONFIG)?
        .eq("pushConfigId", Value::string(config_id))
        .disable_authz()
        .one()?
    else {
        return Ok(None);
    };
    let created = config.get("createdDate").and_then(millis_of).unwrap_or(0);
    let events = facade
        .find(RUN_EVENT)?
        .eq("runId", Value::string(task.id()))
        .eq("eventType", Value::string("run.state"))
        .condition(Condition::greater("eventSeq", Value::I64(cursor)))
        .order_by("eventSeq", false)
        .limit(PROJECT_PAGE)
        .disable_authz()
        .list()?;
    let base_url = settings(&config)
        .1
        .unwrap_or_else(|| super::card::base_url(factory, &Origin::default()));
    let mut projector = Projector::new(Follow::of(task, &base_url, false, false));
    let mut last = cursor;
    for event in events.values() {
        let stored = run::events::StoredEvent::of(event);
        last = last.max(stored.seq);
        // a state before the config was made is not pushed
        if stored.date_millis.is_some_and(|at| at < created - 1000) {
            continue;
        }
        let (out, _) = projector.project(facade, &stored.event_type, &stored.payload)?;
        let kept = in_tx(facade, || {
            // made once: under the config's row lock, and only when no row has this event yet
            if facade
                .find(CONFIG)?
                .eq("pushConfigId", Value::string(config_id))
                .for_update(true)
                .disable_authz()
                .one()?
                .is_none()
            {
                return Ok(false);
            }
            let made = facade
                .find(DELIVERY)?
                .eq("pushConfigId", Value::string(config_id))
                .eq("eventSeq", Value::I64(stored.seq))
                .disable_authz()
                .count()?;
            if made > 0 {
                return Ok(true);
            }
            for response in &out {
                let mut row = facade.make_value(DELIVERY)?;
                row.set_sequenced_id_primary(facade)?;
                row.set("pushConfigId", Value::string(config_id));
                row.set("runId", Value::string(task.id()));
                row.set("eventSeq", Value::I64(stored.seq));
                row.set("payloadJson", Value::string(response.to_string()));
                row.set("attemptCount", Value::I64(0));
                row.set("nextAttemptDate", stamp(now_millis()));
                row.create(facade)?;
            }
            Ok(true)
        })?;
        if !kept {
            return Ok(None);
        }
    }
    // every state event up to the task's newest was read when the page was not full
    if i64::try_from(events.values().len()).unwrap_or(0) < PROJECT_PAGE {
        last = last.max(task.last_event_seq);
    }
    Ok(Some(last))
}

/// Remove the push rows past [`CONFIG_DAYS`]: the configs made before then with all their notifications, and the
/// notifications delivered or failed before then. Out: the configs and the notifications removed.
///
/// # Errors
///
/// Returns an error when the rows cannot be removed.
pub fn clean(facade: &EntityFacade, now: i64) -> Result<(i64, i64), Error> {
    let cutoff = stamp(now - CONFIG_DAYS * DAY);
    in_tx(facade, || {
        let old: Vec<Value> = facade
            .find(CONFIG)?
            .condition(Condition::less("createdDate", cutoff.clone()))
            .select_field("pushConfigId")
            .limit(1000)
            .disable_authz()
            .list()?
            .values()
            .iter()
            .filter_map(|row| text_of(row, "pushConfigId").map(Value::string))
            .collect();
        let mut deliveries = 0;
        let mut configs = 0;
        if !old.is_empty() {
            deliveries += facade
                .find(DELIVERY)?
                .condition(Condition::is_in("pushConfigId", old.clone()))
                .disable_authz()
                .delete_all()?;
            configs += facade
                .find(CONFIG)?
                .condition(Condition::is_in("pushConfigId", old))
                .disable_authz()
                .delete_all()?;
        }
        for field in ["deliveredDate", "failedDate"] {
            deliveries += facade
                .find(DELIVERY)?
                .condition(Condition::less(field, cutoff.clone()))
                .disable_authz()
                .delete_all()?;
        }
        Ok((configs, deliveries))
    })
}

/// The two clients of the posts: one that refuses internal addresses, and one for the hosts `allow-hosts` names. Neither
/// follows a redirect, and a connection has [`CONNECT_TIMEOUT`] to open.
fn client(open: bool) -> &'static RestClient {
    static PUBLIC: OnceLock<RestClient> = OnceLock::new();
    static OPEN: OnceLock<RestClient> = OnceLock::new();
    let slot = if open { &OPEN } else { &PUBLIC };
    slot.get_or_init(|| {
        RestClient::new()
            .deny_internal_addresses(!open)
            .redirects(0, false)
            .connect_timeout(CONNECT_TIMEOUT)
            .max_response_bytes(RESPONSE_CAP)
            .truncate_body(true)
    })
}

/// A waiting notification as the outbox orders it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Waiting {
    pub(crate) config_id: String,
    pub(crate) delivery_id: String,
    pub(crate) next: i64,
}

/// The head of each config (its oldest waiting notification, `waiting` being in the outbox's order), when it is due at
/// `now`; a config whose head waits for a retry sends nothing after it.
pub(crate) fn due_heads(waiting: &[Waiting], now: i64) -> Vec<Waiting> {
    let mut seen = std::collections::HashSet::new();
    waiting
        .iter()
        .filter(|row| seen.insert(row.config_id.clone()))
        .filter(|row| row.next <= now)
        .cloned()
        .collect()
}

/// The head row of `config_id`, when there is one and it is due.
fn head_of(facade: &EntityFacade, config_id: &str, now: i64) -> Result<Option<EntityValue>, Error> {
    let head = facade
        .find(DELIVERY)?
        .eq("pushConfigId", Value::string(config_id))
        .condition(Condition::is_null("deliveredDate"))
        .condition(Condition::is_null("failedDate"))
        .order_by("eventSeq", false)
        .order_by("deliveryId", false)
        .limit(1)
        .disable_authz()
        .list()?
        .values()
        .first()
        .cloned();
    Ok(head.filter(|row| {
        row.get("nextAttemptDate")
            .and_then(millis_of)
            .is_none_or(|at| at <= now)
    }))
}

/// Send the head of each config that has one due (read are only the configs with a due row, and only their heads),
/// and the rows after it while they get through, until `deadline`. [`WORKERS`] threads send, at most
/// [`PER_HOST`] at once to one host, so a slow or dead receiver holds up only its own configs.
fn deliver(factory: &Arc<ExecutionContextFactory>, facade: &EntityFacade, deadline: Instant) -> Result<usize, Error> {
    let now = now_millis();
    let mut config_ids: Vec<String> = Vec::new();
    for row in facade
        .find(DELIVERY)?
        .condition(Condition::is_null("deliveredDate"))
        .condition(Condition::is_null("failedDate"))
        .condition(Condition::less_equal("nextAttemptDate", stamp(now)))
        .select_field("pushConfigId")
        .order_by("nextAttemptDate", false)
        .limit(1000)
        .disable_authz()
        .list()?
        .values()
    {
        let id = text_of(row, "pushConfigId").unwrap_or_default();
        if !config_ids.contains(&id) && config_ids.len() < MAX_CONFIGS_PER_ROUND {
            config_ids.push(id);
        }
    }
    if config_ids.is_empty() {
        return Ok(0);
    }
    let ids: Vec<Value> = config_ids.iter().map(Value::string).collect();
    let waiting: Vec<Waiting> = facade
        .find(DELIVERY)?
        .condition(Condition::is_in("pushConfigId", ids.clone()))
        .condition(Condition::is_null("deliveredDate"))
        .condition(Condition::is_null("failedDate"))
        .select_field("pushConfigId")
        .select_field("deliveryId")
        .select_field("nextAttemptDate")
        .order_by("pushConfigId", false)
        .order_by("eventSeq", false)
        .order_by("deliveryId", false)
        .disable_authz()
        .list()?
        .values()
        .iter()
        .map(|row| Waiting {
            config_id: text_of(row, "pushConfigId").unwrap_or_default(),
            delivery_id: text_of(row, "deliveryId").unwrap_or_default(),
            next: row.get("nextAttemptDate").and_then(millis_of).unwrap_or(0),
        })
        .collect();
    let heads = due_heads(&waiting, now);
    if heads.is_empty() {
        return Ok(0);
    }
    let configs: HashMap<String, EntityValue> = facade
        .find(CONFIG)?
        .condition(Condition::is_in("pushConfigId", ids))
        .disable_authz()
        .list()?
        .values()
        .iter()
        .map(|row| (text_of(row, "pushConfigId").unwrap_or_default(), row.clone()))
        .collect();
    let jobs: VecDeque<(String, EntityValue)> = heads
        .into_iter()
        .filter_map(|head| {
            let config = configs.get(&head.config_id)?.clone();
            let host = scheme_host(&text_of(&config, "url").unwrap_or_default()).map(|(_, host)| host)?;
            Some((host, config))
        })
        .collect();
    let llm = crate::llm::facade(factory);
    let conf = llm.conf().a2a.push.clone();
    let queue = Mutex::new(jobs);
    let busy: Mutex<HashMap<String, usize>> = Mutex::new(HashMap::new());
    let delivered = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..WORKERS {
            scope.spawn(|| {
                let context = factory.new_context();
                let _entered = context.enter();
                let _authz = context.disable_authz();
                loop {
                    if Instant::now() >= deadline {
                        return;
                    }
                    // the next config whose host has room, else wait for one, else stop
                    let job = {
                        let mut queue = lock(&queue);
                        if queue.is_empty() {
                            return;
                        }
                        let mut busy = lock(&busy);
                        let found = queue
                            .iter()
                            .position(|(host, _)| busy.get(host).copied().unwrap_or(0) < PER_HOST);
                        found.and_then(|at| queue.remove(at)).inspect(|(host, _)| {
                            *busy.entry(host.clone()).or_insert(0) += 1;
                        })
                    };
                    let Some((host, config)) = job else {
                        std::thread::sleep(Duration::from_millis(20));
                        continue;
                    };
                    let config_id = text_of(&config, "pushConfigId").unwrap_or_default();
                    while Instant::now() < deadline {
                        let next = quietly(|| head_of(facade, &config_id, now_millis()));
                        let Ok(Some(row)) = next else {
                            if let Err(error) = next {
                                tracing::error!(
                                    "the push notifications of config {config_id} could not be read: {error}"
                                );
                            }
                            break;
                        };
                        match quietly(|| deliver_one(factory, facade, &config, &row, &conf)) {
                            Ok(true) => {
                                delivered.fetch_add(1, Ordering::SeqCst);
                            }
                            // someone else has it, or it failed: the rows after it wait
                            Ok(false) => break,
                            Err(error) => {
                                tracing::error!(
                                    "push notification {} of config {config_id} could not be sent: {error}",
                                    text_of(&row, "deliveryId").unwrap_or_default()
                                );
                                break;
                            }
                        }
                    }
                    if let Some(count) = lock(&busy).get_mut(&host) {
                        *count = count.saturating_sub(1);
                    }
                }
            });
        }
    });
    Ok(delivered.load(Ordering::SeqCst))
}

/// Send one row when it is due and this node claims it. Out: whether it got through.
fn deliver_one(
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    config: &EntityValue,
    row: &EntityValue,
    conf: &crate::llm::conf::A2aPushConf,
) -> Result<bool, Error> {
    let now = now_millis();
    if row
        .get("nextAttemptDate")
        .and_then(millis_of)
        .is_some_and(|at| at > now)
    {
        return Ok(false);
    }
    let delivery_id = text_of(row, "deliveryId").unwrap_or_default();
    let attempts = int_of(row, "attemptCount").unwrap_or(0);
    let timeout = Duration::from_secs(conf.timeout.max(1));
    let claimed = in_tx(facade, || {
        facade
            .find(DELIVERY)?
            .eq("deliveryId", Value::string(&delivery_id))
            .eq("attemptCount", Value::I64(attempts))
            .condition(Condition::is_null("deliveredDate"))
            .disable_authz()
            .update_all([
                ("attemptCount".to_string(), Value::I64(attempts + 1)),
                (
                    "nextAttemptDate".to_string(),
                    stamp(now + i64::try_from(timeout.as_millis()).unwrap_or(15_000) + 30_000),
                ),
            ])
    })?;
    if claimed == 0 {
        return Ok(false);
    }
    let url = text_of(config, "url").unwrap_or_default();
    let open = scheme_host(&url).is_some_and(|(_, host)| allowed_host(factory, &host));
    let mut call = client(open)
        .post(&url)
        .header("A2A-Version", types::VERSION)
        .text("application/json", text_of(row, "payloadJson").unwrap_or_default())
        .timeout(timeout);
    if let Some(token) = text_of(config, "token") {
        call = call.header("X-A2A-Notification-Token", token);
    }
    if let (Some((scheme, Some(credentials))), _) = settings(config) {
        call = call.header("Authorization", format!("{scheme} {credentials}"));
    }
    let outcome = call.send(false);
    let attempt = attempts + 1;
    let (status, error) = match &outcome {
        Ok(response) if (200..300).contains(&response.status()) => (Some(response.status()), None),
        Ok(response) => (
            Some(response.status()),
            Some(format!("the receiver answered {}", response.status())),
        ),
        Err(error) => (None, Some(crate::llm::usage::redact(&error.to_string(), &[]))),
    };
    let max_attempts = i64::from(conf.max_attempts.max(1));
    in_tx(facade, || {
        let mut fields = vec![(
            "lastStatus".to_string(),
            status.map_or(Value::Null, |status| Value::I64(i64::from(status))),
        )];
        match &error {
            None => fields.push(("deliveredDate".to_string(), stamp(now_millis()))),
            Some(text) => {
                fields.push(("lastError".to_string(), Value::string(text)));
                if attempt >= max_attempts {
                    fields.push(("failedDate".to_string(), stamp(now_millis())));
                } else {
                    let wait = BACKOFF[usize::try_from(attempt - 1).unwrap_or(0).min(BACKOFF.len() - 1)];
                    fields.push(("nextAttemptDate".to_string(), stamp(now_millis() + wait)));
                }
            }
        }
        facade
            .find(DELIVERY)?
            .eq("deliveryId", Value::string(&delivery_id))
            .disable_authz()
            .update_all(fields)
    })?;
    match error {
        None => Ok(true),
        Some(text) => {
            tracing::warn!(
                "push notification {delivery_id} to {} failed (attempt {attempt} of {max_attempts}): {text}",
                moqui_lib::http::redacted(&url)
            );
            // a row that will not be tried again lets the next one go in the next round
            Ok(false)
        }
    }
}
