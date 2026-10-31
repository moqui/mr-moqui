//! Carrying out a recovery action (plans/Monitoring.md, Later: Recovery). The action row is saved before anything
//! runs, keyed by an idempotency key, while the action holds its scope's lease, so one action is active per scope and
//! a retried run reaches the controller with the same key. An in-process action is done at once and keeps what
//! reverting it restores; a webhook posts a signed request to a deployment controller, which answers at once or reports
//! back on `/v1/recovery` (or is polled at its `statusUrl`); a manual action waits for an operator. An action past its
//! deadline is timed out and escalated.

use super::policy::{Action, AdapterKind};
use super::{fail, Error, ExecutionContext, Value};
use crate::incident;
use crate::regression::{field, millis, stamp};
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::transaction::TransactionAttribute;
use serde_json::{json, Value as Json};

pub(crate) const ACTION: &str = "moqui.monitor.RecoveryAction";
pub(crate) const ADAPTER: &str = "moqui.monitor.RecoveryAdapter";
/// Statuses of an action that holds its scope.
pub(crate) const ACTIVE: [&str; 3] = ["Persisted", "Running", "AwaitingOperator"];
/// Webhook attempts before an action waits only for its deadline.
const MAX_ATTEMPTS: i64 = 5;

/// A RecoveryAdapter row, read.
#[derive(Clone, Debug)]
pub(crate) struct Adapter {
    pub id: String,
    pub kind: AdapterKind,
    pub url: String,
    pub status_url: String,
    pub secret_property: String,
    pub headers: Vec<(String, String)>,
    pub allow_internal: bool,
    pub instructions: String,
}

pub(crate) fn adapters(entity: &EntityFacade) -> Result<Vec<Adapter>, Error> {
    let rows = entity.find(ADAPTER)?.list()?;
    Ok(rows
        .values()
        .iter()
        .filter_map(|row| {
            let kind = AdapterKind::parse(&field(row, "adapterType"))?;
            let url = match field(row, "urlProperty") {
                property if !property.is_empty() => moqui_lib::env::get_property(&property).unwrap_or_default(),
                _ => field(row, "url"),
            };
            let headers = serde_json::from_str::<serde_json::Map<String, Json>>(&field(row, "headerPropertiesJson"))
                .map(|map| {
                    map.into_iter()
                        .filter_map(|(header, property)| Some((header, property.as_str()?.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            Some(Adapter {
                id: field(row, "adapterId"),
                kind,
                url,
                status_url: field(row, "statusUrl"),
                secret_property: field(row, "secretProperty"),
                headers,
                allow_internal: field(row, "allowInternal") == "Y",
                instructions: field(row, "instructions"),
            })
        })
        .collect())
}

fn sha256(text: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// What one run of an action's adapter did.
enum Ran {
    Succeeded {
        undo: Option<Json>,
        state: String,
    },
    Running {
        external: Option<String>,
        note: String,
    },
    AwaitingOperator,
    Failed(String),
    /// The controller could not be reached or refused: tried again by a later pass, until the deadline.
    Retry(String),
}

/// Start the action a decision chose: the existing action when this decision already started one, None when another
/// action holds the scope. The action is saved before its adapter runs.
#[allow(clippy::too_many_arguments)]
pub(crate) fn start(
    ec: &ExecutionContext,
    entity: &EntityFacade,
    decision: &EntityValue,
    incident: &EntityValue,
    contact: &Json,
    deadline: i64,
    verify_after: i64,
    now: i64,
) -> Result<Option<EntityValue>, Error> {
    let _authz = ec.disable_authz();
    let decision_id = field(decision, "decisionId");
    let key = sha256(&format!("recovery-action\n{decision_id}"));
    if let Some(existing) = entity.find(ACTION)?.eq("idempotencyKey", Value::string(&key)).one()? {
        return Ok(Some(existing));
    }
    let scope = field(decision, "scopeKey");
    let action = field(decision, "action");
    // Escalation changes nothing, so it does not hold the scope.
    let lease = if action == Action::Escalate.as_str() {
        None
    } else {
        match crate::lock::claim(ec, entity, &scope, deadline.max(60_000))? {
            Some(lease) => Some(lease),
            None => return Ok(None),
        }
    };
    let token = moqui_core::user::random_string(32).map_err(fail)?;
    let tx = entity.transactions_for(ACTION)?;
    let mut row = tx.run(TransactionAttribute::ForceNew, None, || {
        let mut row = entity.make_value(ACTION)?;
        row.set_sequenced_id_primary(entity)?;
        for name in ["decisionId", "incidentId", "action", "target", "adapterId", "scopeKey"] {
            row.set(name, decision.get(name).cloned().unwrap_or(Value::Null));
        }
        row.set("idempotencyKey", Value::string(&key));
        row.set("callbackTokenHash", Value::String(sha256(&token)));
        row.set("leaseToken", lease.clone().map_or(Value::Null, Value::String));
        row.set("statusId", Value::string("Persisted"));
        row.set("attempts", Value::I64(0));
        row.set("createdDate", stamp(now));
        row.set("deadlineDate", stamp(now + deadline));
        row.set("verifyAfterDate", stamp(now + verify_after));
        row.create(entity)?;
        Ok(row)
    })?;
    incident::add_evidence(
        entity,
        &field(&row, "incidentId"),
        now,
        "action",
        &field(&row, "actionId"),
        &format!("{action} {} started", field(&row, "target")),
        None,
    )?;
    attempt(ec, entity, &mut row, incident, contact, Some(&token), now)?;
    Ok(Some(row))
}

/// Run (or run again) a persisted action's adapter and save what it did.
pub(crate) fn attempt(
    ec: &ExecutionContext,
    entity: &EntityFacade,
    row: &mut EntityValue,
    incident: &EntityValue,
    contact: &Json,
    token: Option<&str>,
    now: i64,
) -> Result<(), Error> {
    let _authz = ec.disable_authz();
    let action = Action::parse(&field(row, "action")).ok_or_else(|| fail("unknown recovery action"))?;
    let adapter_id = field(row, "adapterId");
    let adapter = adapters(entity)?.into_iter().find(|adapter| adapter.id == adapter_id);
    let attempts = millis(row.get("attempts")).unwrap_or(0) + 1;
    row.set("attempts", Value::I64(attempts));
    let ran = match (action, adapter.as_ref().map(|adapter| adapter.kind)) {
        (Action::Escalate, _) => escalate(incident, contact, "Incident needs attention"),
        (_, Some(AdapterKind::InProcess)) => {
            in_process(ec, entity, action, &field(row, "target"), &field(row, "actionId"))
        }
        (_, Some(AdapterKind::Manual)) => {
            if let Some(adapter) = &adapter {
                row.set(
                    "requestJson",
                    Value::String(json!({"instructions": adapter.instructions}).to_string()),
                );
            }
            Ran::AwaitingOperator
        }
        (_, Some(AdapterKind::Webhook)) => {
            let adapter = adapter.as_ref().expect("a webhook kind came from an adapter");
            webhook(adapter, row, incident, token)
        }
        (_, None) => Ran::Failed(format!("adapter {adapter_id} does not exist")),
    };
    apply(ec, entity, row, ran, now, attempts)
}

/// Save what a run did; an action that ended gives its scope back.
fn apply(
    ec: &ExecutionContext,
    entity: &EntityFacade,
    row: &mut EntityValue,
    ran: Ran,
    now: i64,
    attempts: i64,
) -> Result<(), Error> {
    let incident_id = field(row, "incidentId");
    let action_id = field(row, "actionId");
    let summary = match ran {
        Ran::Succeeded { undo, state } => {
            row.set("statusId", Value::string("Succeeded"));
            row.set("effectiveDate", stamp(now));
            if let Some(undo) = undo {
                row.set("undoJson", Value::String(undo.to_string()));
            }
            row.set("resultingState", Value::string(&state));
            finish(ec, entity, row, now);
            format!("{} {} took effect: {state}", field(row, "action"), field(row, "target"))
        }
        Ran::Running { external, note } => {
            row.set("statusId", Value::string("Running"));
            if let Some(external) = external {
                row.set("externalId", Value::String(external));
            }
            row.set("responseText", Value::string(&note));
            format!("{} accepted by the controller", field(row, "action"))
        }
        Ran::AwaitingOperator => {
            row.set("statusId", Value::string("AwaitingOperator"));
            format!("{} waits for an operator to carry it out", field(row, "action"))
        }
        Ran::Failed(reason) => {
            row.set("statusId", Value::string("Failed"));
            row.set("responseText", Value::string(&reason));
            finish(ec, entity, row, now);
            format!("{} failed: {reason}", field(row, "action"))
        }
        Ran::Retry(reason) => {
            row.set("responseText", Value::string(&reason));
            format!(
                "{} attempt {attempts} did not reach the controller: {reason}",
                field(row, "action")
            )
        }
    };
    row.update(entity)?;
    incident::add_evidence(entity, &incident_id, now, "action", &action_id, &summary, None)?;
    tracing::info!(target: incident::AUDIT, action = %action_id, status = %field(row, "statusId"), "recovery action run");
    Ok(())
}

/// An action ended: its finish time, and its scope given back.
fn finish(ec: &ExecutionContext, entity: &EntityFacade, row: &mut EntityValue, now: i64) {
    row.set("finishedDate", stamp(now));
    let lease = field(row, "leaseToken");
    if !lease.is_empty() {
        crate::lock::release(ec, entity, &field(row, "scopeKey"), &lease);
    }
}

fn escalate(incident: &EntityValue, contact: &Json, title: &str) -> Ran {
    let id = field(incident, "incidentId");
    let payload = json!({
        "title": format!("{title}: {}", field(incident, "title")),
        "link": format!("/apps/monitor/IncidentDetail?incidentId={id}"),
        "incidentId": id,
        "severity": field(incident, "severity"),
        "status": field(incident, "statusId"),
        "serviceName": field(incident, "serviceName"),
    });
    match crate::alert::send_contact(contact, &payload) {
        Ok(()) => Ran::Succeeded {
            undo: None,
            state: "escalated".into(),
        },
        Err(error) => Ran::Failed(format!("escalation was not delivered: {error}")),
    }
}

/// Tell the policy's contact point about an incident, logging a failure: approval requests and timeouts.
pub(crate) fn notify(incident: &EntityValue, contact: &Json, title: &str) {
    if let Ran::Failed(error) = escalate(incident, contact, title) {
        tracing::error!(incident = %field(incident, "incidentId"), %error, "could not send a recovery notification");
    }
}

/// The actions MR Moqui can do itself, each with what reverting it restores.
fn in_process(ec: &ExecutionContext, entity: &EntityFacade, action: Action, target: &str, action_id: &str) -> Ran {
    let done = (|| -> Result<Ran, Error> {
        Ok(match action {
            Action::PauseJob => {
                let mut job = entity
                    .find("moqui.service.job.ServiceJob")?
                    .eq("jobName", Value::string(target))
                    .one()?
                    .ok_or_else(|| fail(format!("no ServiceJob {target}")))?;
                let was = field(&job, "paused");
                job.set("paused", Value::string("Y"));
                job.update(entity)?;
                Ran::Succeeded {
                    undo: Some(json!({"jobName": target, "paused": if was.is_empty() { "N" } else { was.as_str() }})),
                    state: format!("job {target} paused"),
                }
            }
            Action::PauseAlertRule => {
                let mut rule = entity
                    .find("moqui.report.Definition")?
                    .eq("definitionId", Value::string(target))
                    .one()?
                    .ok_or_else(|| fail(format!("no alert rule {target}")))?;
                let mut body: Json = serde_json::from_str(&field(&rule, "bodyJson")).unwrap_or_else(|_| json!({}));
                let was = body["paused"].as_bool().unwrap_or(false);
                body["paused"] = json!(true);
                rule.set("bodyJson", Value::String(body.to_string()));
                rule.update(entity)?;
                Ran::Succeeded {
                    undo: Some(json!({"definitionId": target, "paused": was})),
                    state: format!("alert rule {target} paused"),
                }
            }
            Action::DisableArtifact => {
                if entity
                    .find("moqui.security.ArtifactGroup")?
                    .eq("artifactGroupId", Value::string(target))
                    .one()?
                    .is_none()
                {
                    return Ok(Ran::Failed(format!("no artifact group {target}")));
                }
                let authz_id = format!("RECOVERY_{action_id}");
                let mut authz = entity.make_value("moqui.security.ArtifactAuthz")?;
                authz.set("artifactAuthzId", Value::string(&authz_id));
                authz.set("artifactGroupId", Value::string(target));
                authz.set("userGroupId", Value::string("ALL_USERS"));
                authz.set("authzTypeEnumId", Value::string("AUTHZT_DENY"));
                authz.set("authzActionEnumId", Value::string("AUTHZA_ALL"));
                authz.create(entity)?;
                Ran::Succeeded {
                    undo: Some(json!({"artifactAuthzId": authz_id})),
                    state: format!("artifact group {target} denied to all users"),
                }
            }
            Action::IsolateInstance => {
                moqui_core::drain::set_drained(
                    ec.factory(),
                    target,
                    true,
                    &format!("recovery action {action_id}"),
                    Some(action_id),
                )?;
                Ran::Succeeded {
                    undo: Some(json!({"instanceId": target})),
                    state: format!("instance {target} drained"),
                }
            }
            other => Ran::Failed(format!("{} is not done in process", other.as_str())),
        })
    })();
    done.unwrap_or_else(|error| Ran::Failed(error.to_string()))
}

/// Undo what an in-process action did.
pub(crate) fn revert(ec: &ExecutionContext, entity: &EntityFacade, row: &EntityValue) -> Result<String, Error> {
    let undo: Json =
        serde_json::from_str(&field(row, "undoJson")).map_err(|_| fail("this action has nothing to revert"))?;
    match Action::parse(&field(row, "action")) {
        Some(Action::PauseJob) => {
            let name = undo["jobName"].as_str().unwrap_or("");
            let mut job = entity
                .find("moqui.service.job.ServiceJob")?
                .eq("jobName", Value::string(name))
                .one()?
                .ok_or_else(|| fail(format!("no ServiceJob {name}")))?;
            job.set("paused", Value::string(undo["paused"].as_str().unwrap_or("N")));
            job.update(entity)?;
            Ok(format!("job {name} restored"))
        }
        Some(Action::PauseAlertRule) => {
            let id = undo["definitionId"].as_str().unwrap_or("");
            let mut rule = entity
                .find("moqui.report.Definition")?
                .eq("definitionId", Value::string(id))
                .one()?
                .ok_or_else(|| fail(format!("no alert rule {id}")))?;
            let mut body: Json = serde_json::from_str(&field(&rule, "bodyJson")).unwrap_or_else(|_| json!({}));
            body["paused"] = undo["paused"].clone();
            rule.set("bodyJson", Value::String(body.to_string()));
            rule.update(entity)?;
            Ok(format!("alert rule {id} restored"))
        }
        Some(Action::DisableArtifact) => {
            let id = undo["artifactAuthzId"].as_str().unwrap_or("");
            if let Some(authz) = entity
                .find("moqui.security.ArtifactAuthz")?
                .eq("artifactAuthzId", Value::string(id))
                .one()?
            {
                authz.delete(entity)?;
            }
            Ok(format!("authorization {id} removed"))
        }
        Some(Action::IsolateInstance) => {
            let instance = undo["instanceId"].as_str().unwrap_or("");
            moqui_core::drain::set_drained(ec.factory(), instance, false, "recovery action reverted", None)?;
            Ok(format!("instance {instance} back in service"))
        }
        _ => Err(fail(
            "only in-process actions can be reverted here; ask the controller to undo the others",
        )),
    }
}

/// The signed request a deployment controller gets.
fn webhook(adapter: &Adapter, row: &EntityValue, incident: &EntityValue, token: Option<&str>) -> Ran {
    if adapter.url.is_empty() {
        return Ran::Failed(format!("webhook adapter {} has no URL", adapter.id));
    }
    let mut body = json!({
        "actionId": field(row, "actionId"),
        "idempotencyKey": field(row, "idempotencyKey"),
        "incidentId": field(row, "incidentId"),
        "action": field(row, "action"),
        "target": field(row, "target"),
        "serviceName": field(incident, "serviceName"),
        "version": field(incident, "version"),
        "callbackPath": "/v1/recovery",
    });
    // The token goes only with the first attempt; a retry carries the same idempotency key, and the controller keeps
    // the token it was sent.
    if let Some(token) = token {
        body["callbackToken"] = json!(token);
    }
    let text = body.to_string();
    let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
    if !adapter.secret_property.is_empty() {
        let Some(secret) = moqui_lib::env::get_property(&adapter.secret_property) else {
            return Ran::Failed(format!("secret property {} is not set", adapter.secret_property));
        };
        use hmac::Mac;
        let Ok(mut mac) = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()) else {
            return Ran::Failed("the webhook secret cannot key an HMAC".into());
        };
        mac.update(text.as_bytes());
        let signature: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        headers.push(("x-moqui-signature".into(), format!("sha256={signature}")));
    }
    for (header, property) in &adapter.headers {
        match moqui_lib::env::get_property(property) {
            Some(value) if !value.contains(['\r', '\n']) => headers.push((header.clone(), value)),
            _ => return Ran::Failed(format!("header property {property} is not set")),
        }
    }
    let response = {
        let _suppressed = moqui_core::telemetry::suppress();
        moqui_core::outbound::send(moqui_core::outbound::Request {
            method: moqui_core::outbound::Method::Post,
            url: adapter.url.clone(),
            headers,
            body: Some(text),
            timeout: Some(std::time::Duration::from_secs(10)),
            deny_internal: !adapter.allow_internal,
        })
    };
    match response {
        Err(error) => Ran::Retry(error.to_string()),
        Ok(response) if !(200..300).contains(&response.status) => {
            Ran::Retry(format!("the controller answered {}", response.status))
        }
        Ok(response) => controller_status(&response.body),
    }
}

/// A controller's answer: `{status, externalId, message}`, `succeeded` or `failed` when it is done, else running.
fn controller_status(body: &str) -> Ran {
    let answer: Json = serde_json::from_str(body).unwrap_or(Json::Null);
    let message = answer["message"].as_str().unwrap_or("").to_string();
    match answer["status"].as_str().unwrap_or("") {
        "succeeded" | "done" => Ran::Succeeded {
            undo: None,
            state: if message.is_empty() {
                "done by the controller".into()
            } else {
                message
            },
        },
        "failed" => Ran::Failed(if message.is_empty() {
            "the controller failed it".into()
        } else {
            message
        }),
        _ => Ran::Running {
            external: answer["externalId"].as_str().map(str::to_string),
            note: message,
        },
    }
}

/// Move an active action on: try a webhook again, poll a controller's status, time out past the deadline.
pub(crate) fn advance(
    ec: &ExecutionContext,
    entity: &EntityFacade,
    row: &mut EntityValue,
    incident: &EntityValue,
    contact: &Json,
    now: i64,
) -> Result<(), Error> {
    let status = field(row, "statusId");
    if millis(row.get("deadlineDate")).is_some_and(|deadline| now > deadline) {
        row.set("statusId", Value::string("TimedOut"));
        finish(ec, entity, row, now);
        row.update(entity)?;
        let summary = format!("{} {} timed out", field(row, "action"), field(row, "target"));
        incident::add_evidence(
            entity,
            &field(row, "incidentId"),
            now,
            "action",
            &field(row, "actionId"),
            &summary,
            None,
        )?;
        notify(incident, contact, "Recovery action timed out");
        return Ok(());
    }
    let attempts = millis(row.get("attempts")).unwrap_or(0);
    match status.as_str() {
        "Persisted" if attempts < MAX_ATTEMPTS => attempt(ec, entity, row, incident, contact, None, now),
        "Running" => {
            let adapter_id = field(row, "adapterId");
            let Some(adapter) = adapters(entity)?.into_iter().find(|adapter| adapter.id == adapter_id) else {
                return Ok(());
            };
            if adapter.status_url.is_empty() {
                return Ok(());
            }
            let separator = if adapter.status_url.contains('?') { '&' } else { '?' };
            let url = format!(
                "{}{separator}actionId={}&idempotencyKey={}",
                adapter.status_url,
                field(row, "actionId"),
                field(row, "idempotencyKey")
            );
            let response = {
                let _suppressed = moqui_core::telemetry::suppress();
                moqui_core::outbound::send(moqui_core::outbound::Request {
                    method: moqui_core::outbound::Method::Get,
                    url,
                    headers: Vec::new(),
                    body: None,
                    timeout: Some(std::time::Duration::from_secs(10)),
                    deny_internal: !adapter.allow_internal,
                })
            };
            match response {
                Ok(response) if (200..300).contains(&response.status) => match controller_status(&response.body) {
                    Ran::Running { .. } => Ok(()),
                    done => apply(ec, entity, row, done, now, attempts),
                },
                Ok(response) => {
                    tracing::warn!(action = %field(row, "actionId"), status = response.status, "recovery status poll refused");
                    Ok(())
                }
                Err(error) => {
                    tracing::warn!(action = %field(row, "actionId"), %error, "recovery status poll failed");
                    Ok(())
                }
            }
        }
        _ => Ok(()),
    }
}

/// What a controller or an operator says about an action.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Report<'a> {
    /// The action's callback token, which a controller must give; None for an operator.
    pub token: Option<&'a str>,
    pub status: &'a str,
    pub external: Option<&'a str>,
    pub message: &'a str,
}

/// A controller's report on `/v1/recovery`, or an operator marking a manual action: `succeeded`, `failed` or
/// `running`. A controller proves itself with the action's callback token.
pub(crate) fn report(
    ec: &ExecutionContext,
    entity: &EntityFacade,
    action_id: &str,
    report: &Report<'_>,
    now: i64,
) -> Result<EntityValue, Error> {
    let Report {
        token,
        status,
        external,
        message,
    } = *report;
    let _authz = ec.disable_authz();
    let mut row = entity
        .find(ACTION)?
        .eq("actionId", Value::string(action_id))
        .one()?
        .ok_or_else(|| fail(format!("no recovery action {action_id}")))?;
    if let Some(token) = token {
        if sha256(token) != field(&row, "callbackTokenHash") {
            return Err(fail("the callback token does not match the action"));
        }
    }
    if !ACTIVE.contains(&field(&row, "statusId").as_str()) {
        return Err(fail(format!(
            "recovery action {action_id} is {}, not active",
            field(&row, "statusId")
        )));
    }
    if let Some(external) = external.filter(|external| !external.is_empty()) {
        row.set("externalId", Value::string(external));
    }
    let ran = match status {
        "succeeded" | "done" => Ran::Succeeded {
            undo: None,
            state: if message.is_empty() {
                "done".into()
            } else {
                message.to_string()
            },
        },
        "failed" => Ran::Failed(if message.is_empty() {
            "reported failed".into()
        } else {
            message.to_string()
        }),
        "running" => Ran::Running {
            external: None,
            note: message.to_string(),
        },
        other => return Err(fail(format!("status {other} is not succeeded, failed or running"))),
    };
    let attempts = millis(row.get("attempts")).unwrap_or(0);
    apply(ec, entity, &mut row, ran, now, attempts)?;
    Ok(row)
}

/// Cancel an active action, giving its scope back. A webhook controller is not told: cancel it there too.
pub(crate) fn cancel(
    ec: &ExecutionContext,
    entity: &EntityFacade,
    row: &mut EntityValue,
    now: i64,
) -> Result<(), Error> {
    if !ACTIVE.contains(&field(row, "statusId").as_str()) {
        return Err(fail("only an active action can be cancelled"));
    }
    row.set("statusId", Value::string("Cancelled"));
    finish(ec, entity, row, now);
    row.update(entity)
}
