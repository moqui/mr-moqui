//! Recovery (plans/Monitoring.md, Later: Recovery; requirements 6 and 7): a durable policy, decision, execution and
//! verification state machine. Each pass of `run#Recovery` moves active actions on ([`execute::advance`]), verifies
//! the actions whose verify time has come ([`verify`]), and for each open incident with a matching `RecoveryPolicy`
//! records a decision ([`policy::decide`]) when what it would decide has changed. Advisory by default: an action runs
//! only when approved (`MONITOR_RECOVER`), or under an automatic policy with `telemetry/recovery allow-automatic`, and
//! never while the emergency stop is on. Escalation, which changes nothing, always runs. Policies, adapters and the
//! stop need `MONITOR_RECOVER_ADMIN`; every change is logged on the `moqui.monitor.audit` target.

pub mod execute;
pub mod policy;
pub mod verify;

use super::{fail, input, output, Error, ExecutionContext, Parameters, ServiceInfo, Value};
use crate::incident::{self, AUDIT, INCIDENT};
use crate::regression::{self, field, millis, now, row_json, stamp};
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::search::SearchFacade;
use policy::{Action, Compatibility, Facts, Mode, Past, Policy, Step};
use serde_json::{json, Value as Json};

const SETTINGS: &str = "telemetry/recovery";
const POLICY: &str = "moqui.monitor.RecoveryPolicy";
const DECISION: &str = "moqui.monitor.RecoveryDecision";
const VERIFICATION: &str = "moqui.monitor.RecoveryVerification";
const CONTROL: &str = "moqui.monitor.RecoveryControl";
const MINUTE: i64 = 60_000;
const DAY: i64 = 86_400_000;

pub fn register(registry: &mut moqui_core::native::NativeRegistry) -> Result<(), moqui_core::context::Error> {
    for (name, function) in [
        ("run_recovery", run as moqui_core::native::NativeService),
        ("approve_decision", approve),
        ("refuse_decision", refuse),
        ("complete_action", complete),
        ("cancel_action", cancel),
        ("revert_action", revert),
        ("set_recovery_stop", set_stop),
        ("list_recovery", list),
        ("store_policy", store_policy),
        ("store_adapter", store_adapter),
    ] {
        registry.service_function(&format!("moqui_core::monitor::{name}"), function)?;
    }
    Ok(())
}

/// telemetry/recovery.
struct Settings {
    enabled: bool,
    allow_automatic: bool,
    verify: i64,
    deadline: i64,
}

impl Settings {
    fn read(ec: &ExecutionContext) -> Self {
        let setting = |name: &str| ec.factory().conf_setting(SETTINGS, name);
        let minutes = |name: &str, default: i64| setting(name).trim().parse::<i64>().unwrap_or(default).max(1) * MINUTE;
        Self {
            enabled: setting("enabled") != "false",
            allow_automatic: setting("allow-automatic") == "true",
            verify: minutes("verify-minutes", 15),
            deadline: minutes("deadline-minutes", 30),
        }
    }
}

fn entity_of(ec: &ExecutionContext) -> Result<EntityFacade, Error> {
    ec.factory().entity().ok_or_else(|| fail("no entity facade"))
}

fn require(permission: &str) -> Result<String, Error> {
    let user = moqui_core::user::current_user_id()
        .map_err(fail)?
        .ok_or_else(|| fail("sign in to work on recovery"))?;
    if moqui_core::user::has_permission(permission).map_err(fail)? {
        Ok(user)
    } else {
        Err(fail(format!("{permission} permission is required")))
    }
}

fn stopped(entity: &EntityFacade) -> Result<bool, Error> {
    Ok(entity
        .find(CONTROL)?
        .eq("controlId", Value::string("RECOVERY"))
        .one()?
        .is_some_and(|row| field(&row, "stopped") == "Y"))
}

fn number(row: &EntityValue, name: &str, default: f64) -> f64 {
    row.get(name)
        .map(moqui_lib::object::to_plain_string)
        .and_then(|text| text.parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(default)
}

fn severity_rank(severity: &str) -> u8 {
    match severity {
        "critical" => 3,
        "major" => 2,
        _ => 1,
    }
}

/// The first enabled policy, by sequence, whose service pattern and least severity fit the incident.
fn policy_for(entity: &EntityFacade, incident: &EntityValue) -> Result<Option<(EntityValue, Policy)>, Error> {
    let rows = entity
        .find(POLICY)?
        .order_by("sequenceNum", false)
        .order_by("policyId", false)
        .list()?;
    let service = field(incident, "serviceName");
    let severity = severity_rank(&field(incident, "severity"));
    for row in rows.values() {
        if field(row, "enabled") == "N" {
            continue;
        }
        let pattern = field(row, "serviceName");
        if !pattern.is_empty() {
            match regex::Regex::new(&format!("^(?:{pattern})$")) {
                Ok(pattern) if pattern.is_match(&service) => {}
                Ok(_) => continue,
                Err(error) => {
                    tracing::error!(policy = %field(row, "policyId"), %error, "recovery policy service pattern is invalid");
                    continue;
                }
            }
        }
        let least = field(row, "minSeverity");
        if severity < severity_rank(if least.is_empty() { "major" } else { &least }) {
            continue;
        }
        let steps = serde_json::from_str::<Vec<Json>>(&field(row, "ladderJson"))
            .unwrap_or_default()
            .iter()
            .filter_map(|step| {
                Some(Step {
                    action: Action::parse(step["action"].as_str()?)?,
                    target: step["target"]
                        .as_str()
                        .filter(|text| !text.is_empty())
                        .map(str::to_string),
                    adapter: step["adapterId"]
                        .as_str()
                        .filter(|text| !text.is_empty())
                        .map(str::to_string),
                })
            })
            .collect();
        let adapter = field(row, "adapterId");
        return Ok(Some((
            row.clone(),
            Policy {
                id: field(row, "policyId"),
                mode: Mode::parse(&field(row, "mode")),
                steps,
                adapter: (!adapter.is_empty()).then_some(adapter),
                min_confidence: number(row, "minConfidence", 0.99),
                cooldown: number(row, "cooldownMinutes", 60.0) as i64 * MINUTE,
                max_per_day: number(row, "maxActionsPerDay", 3.0) as usize,
            },
        )));
    }
    Ok(None)
}

/// Where a policy's escalations go; a notification to the ADMIN group when it names none, or with no policy.
fn contact_of(policy: Option<&EntityValue>) -> Json {
    policy
        .and_then(|policy| serde_json::from_str(&field(policy, "contactJson")).ok())
        .filter(Json::is_object)
        .unwrap_or_else(|| json!({"type": "notification", "topic": "MonitorIncident", "userGroupId": "ADMIN"}))
}

fn scope_of(incident: &EntityValue) -> String {
    format!("recovery:{}", field(incident, "serviceName"))
}

/// What a decision for the incident is made from.
fn facts(
    entity: &EntityFacade,
    search: &SearchFacade,
    incident: &EntityValue,
    settings: &Settings,
    now: i64,
) -> Result<Facts, Error> {
    let id = field(incident, "incidentId");
    let regressions = entity
        .find(regression::REGRESSION)?
        .eq("incidentId", Value::string(&id))
        .list()?;
    let open: Vec<&EntityValue> = regressions
        .values()
        .iter()
        .filter(|row| field(row, "statusId") == "Open")
        .collect();
    let split = |text: String| -> Vec<String> {
        text.split(',')
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect()
    };
    // An alert's or an operator's incident has no regression: the rule or the person is the evidence.
    let confidence = if regressions.values().is_empty() {
        1.0
    } else {
        open.iter()
            .map(|row| number(row, "confidence", 0.0))
            .fold(0.0, f64::max)
    };
    let lead = open
        .iter()
        .find(|row| field(row, "locality") == "instance")
        .or(open.first());
    let (locality, affected, healthy) = match lead {
        Some(row) => (
            field(row, "locality"),
            split(field(row, "affectedInstances")),
            split(field(row, "healthyInstances")),
        ),
        None => ("unknown".to_string(), Vec::new(), Vec::new()),
    };
    let causes = entity
        .find("moqui.monitor.MonitorIncidentCause")?
        .eq("incidentId", Value::string(&id))
        .order_by("causeRank", false)
        .list()?;
    let top_cause = causes
        .values()
        .iter()
        .find(|row| field(row, "statusId") != "rejected")
        .map(|row| (field(row, "kind"), number(row, "score", 0.0)));
    let rollback_to = open
        .iter()
        .map(|row| field(row, "baselineVersion"))
        .find(|version| !version.is_empty() && *version != field(incident, "version"));
    let compatibility = match &rollback_to {
        Some(to) => {
            let releases = regression::releases(search, &field(incident, "serviceName"))?;
            let schema = |version: &str| {
                releases
                    .iter()
                    .find(|release| release.version == version)
                    .and_then(|release| release.schema.clone())
            };
            match (schema(&field(incident, "version")), schema(to)) {
                (Some(from), Some(to)) if from == to => Compatibility::Same,
                (Some(_), Some(_)) => Compatibility::Different,
                _ => Compatibility::Unknown,
            }
        }
        None => Compatibility::Unknown,
    };
    let actions = entity
        .find(execute::ACTION)?
        .eq("scopeKey", Value::string(scope_of(incident)))
        .order_by("createdDate", true)
        .limit(200)
        .list()?;
    let history = actions
        .values()
        .iter()
        .filter_map(|row| {
            let at = millis(row.get("createdDate"))?;
            let status = field(row, "statusId");
            (now - at < 2 * DAY && status != "Cancelled").then(|| Past {
                action: Action::parse(&field(row, "action")).unwrap_or(Action::Observe),
                target: Some(field(row, "target")).filter(|target| !target.is_empty()),
                incident: field(row, "incidentId"),
                at,
                active: execute::ACTIVE.contains(&status.as_str()),
            })
        })
        .collect();
    Ok(Facts {
        incident: id,
        now,
        stopped: stopped(entity)?,
        allow_automatic: settings.allow_automatic,
        confidence,
        degraded: open.iter().any(|row| field(row, "dataQuality") == "degraded"),
        instances: affected.len() + healthy.len(),
        locality,
        affected,
        top_cause,
        compatibility,
        rollback_to,
        history,
        adapters: execute::adapters(entity)?
            .into_iter()
            .map(|adapter| (adapter.id, adapter.kind))
            .collect(),
    })
}

/// A decision's identity: an approval is bound to it, and the same facts make no new decision.
fn decision_hash(incident: &EntityValue, outcome: &policy::Outcome, facts: &Facts) -> String {
    let identity = json!({
        "incident": field(incident, "incidentId"),
        "action": outcome.step.action.as_str(),
        "target": outcome.step.target,
        "adapter": outcome.step.adapter,
        "status": outcome.status.as_str(),
        "blocked": outcome.blocked,
        "confidence": format!("{:.3}", facts.confidence),
        "locality": facts.locality,
        "affected": facts.affected,
        "stopped": facts.stopped,
    });
    moqui_lib::json::canonical_sha256(&identity)
}

/// Decide for an incident: a new decision when what would be decided changed, run at once when approved. Returns the
/// decision row, new or the latest.
#[allow(clippy::too_many_arguments)]
fn decide_for(
    ec: &ExecutionContext,
    entity: &EntityFacade,
    search: &SearchFacade,
    settings: &Settings,
    incident: &EntityValue,
    policy_row: &EntityValue,
    policy: &Policy,
    decided_by: &str,
    now: i64,
) -> Result<Option<EntityValue>, Error> {
    let facts = facts(entity, search, incident, settings, now)?;
    let outcome = policy::decide(policy, &facts);
    let hash = decision_hash(incident, &outcome, &facts);
    let id = field(incident, "incidentId");
    let decisions = entity
        .find(DECISION)?
        .eq("incidentId", Value::string(&id))
        .order_by("decidedDate", true)
        .limit(20)
        .list()?;
    if decisions
        .values()
        .first()
        .is_some_and(|latest| field(latest, "decisionHash") == hash)
    {
        return Ok(None);
    }
    for row in decisions.values() {
        if matches!(field(row, "statusId").as_str(), "Proposed" | "AwaitingApproval") {
            let mut row = row.clone();
            row.set("statusId", Value::string("Superseded"));
            row.update(entity)?;
        }
    }
    let considered: Vec<Json> = outcome
        .considered
        .iter()
        .map(|step| json!({"action": step.action.as_str(), "target": step.target, "refused": step.refused}))
        .collect();
    let mut row = entity.make_value(DECISION)?;
    row.set_sequenced_id_primary(entity)?;
    row.set("incidentId", Value::string(&id));
    row.set("policyId", Value::string(&policy.id));
    row.set("action", Value::string(outcome.step.action.as_str()));
    row.set("target", outcome.step.target.clone().map_or(Value::Null, Value::String));
    row.set(
        "adapterId",
        outcome.step.adapter.clone().map_or(Value::Null, Value::String),
    );
    row.set("scopeKey", Value::String(scope_of(incident)));
    row.set("statusId", Value::string(outcome.status.as_str()));
    row.set(
        "blockedReason",
        outcome.blocked.clone().map_or(Value::Null, Value::String),
    );
    row.set("confidence", Value::F64(facts.confidence));
    row.set("blastRadius", Value::string(&outcome.blast_radius));
    row.set(
        "rationaleJson",
        Value::String(
            json!({"considered": considered, "locality": facts.locality,
        "affected": facts.affected, "compatibility": format!("{:?}", facts.compatibility), "topCause": facts.top_cause})
            .to_string(),
        ),
    );
    row.set("decisionHash", Value::string(&hash));
    row.set("decidedBy", Value::string(decided_by));
    row.set("decidedDate", stamp(now));
    row.create(entity)?;
    let summary = format!(
        "Decision: {} {} — {}{}",
        outcome.step.action.as_str(),
        outcome.step.target.clone().unwrap_or_default(),
        outcome.status.as_str(),
        outcome
            .blocked
            .as_ref()
            .map(|why| format!(" ({why})"))
            .unwrap_or_default()
    );
    incident::add_evidence(entity, &id, now, "decision", &field(&row, "decisionId"), &summary, None)?;
    tracing::info!(target: AUDIT, incident = %id, decision = %field(&row, "decisionId"), action = outcome.step.action.as_str(), status = outcome.status.as_str(), "recovery decision");
    let contact = contact_of(Some(policy_row));
    match (outcome.status, outcome.step.action) {
        (_, Action::Observe) => {}
        (policy::Status::AwaitingApproval, _) => execute::notify(incident, &contact, "Recovery approval needed"),
        (policy::Status::Approved | policy::Status::Blocked, _) => {
            run_decision(ec, entity, settings, &row, incident, policy_row, now)?
        }
        (policy::Status::Proposed, _) => {}
    }
    Ok(Some(row))
}

/// Start a decision's action, and mark the incident Mitigating when it changes the system.
fn run_decision(
    ec: &ExecutionContext,
    entity: &EntityFacade,
    settings: &Settings,
    decision: &EntityValue,
    incident: &EntityValue,
    policy_row: &EntityValue,
    now: i64,
) -> Result<(), Error> {
    let deadline = (number(policy_row, "deadlineMinutes", 0.0) as i64 * MINUTE).max(0);
    let verify = (number(policy_row, "verifyMinutes", 0.0) as i64 * MINUTE).max(0);
    let started = execute::start(
        ec,
        entity,
        decision,
        incident,
        &contact_of(Some(policy_row)),
        if deadline > 0 { deadline } else { settings.deadline },
        if verify > 0 { verify } else { settings.verify },
        now,
    )?;
    if started.is_none() {
        tracing::info!(decision = %field(decision, "decisionId"), "another action holds the scope; the decision waits");
        return Ok(());
    }
    if Action::parse(&field(decision, "action")).is_some_and(Action::changes_system)
        && matches!(field(incident, "statusId").as_str(), "Open" | "Reopened")
    {
        let mut incident = incident.clone();
        incident.set("statusId", Value::string("Mitigating"));
        incident.set("lastUpdated", stamp(now));
        incident.update(entity)?;
    }
    Ok(())
}

fn incident_row(entity: &EntityFacade, id: &str) -> Result<EntityValue, Error> {
    entity
        .find(INCIDENT)?
        .eq("incidentId", Value::string(id))
        .one()?
        .ok_or_else(|| fail(format!("no incident {id}")))
}

/// What one pass did.
#[derive(Default)]
struct Pass {
    decisions: usize,
    advanced: usize,
    failed: Vec<String>,
}

/// `run#Recovery`: one pass of the state machine. Each incident and action is its own piece: one that fails is logged
/// and the pass goes on.
fn run(ec: &ExecutionContext, _: &ServiceInfo<'_>, _: &Parameters) -> Result<Parameters, Error> {
    let settings = Settings::read(ec);
    if !settings.enabled {
        return Ok(output(json!({"skipped": "disabled"})));
    }
    let entity = entity_of(ec)?;
    let search = moqui_core::search::current()?;
    let _authz = ec.disable_authz();
    let now = now(ec);
    let mut pass = Pass::default();
    let actions = entity
        .find(execute::ACTION)?
        .order_by("createdDate", false)
        .limit(500)
        .list()?;
    for row in actions.values() {
        let status = field(row, "statusId");
        let id = field(row, "actionId");
        let step = (|| -> Result<bool, Error> {
            let incident = incident_row(&entity, &field(row, "incidentId"))?;
            let contact = contact_of(policy_for(&entity, &incident)?.map(|(row, _)| row).as_ref());
            let mut row = row.clone();
            if execute::ACTIVE.contains(&status.as_str()) {
                execute::advance(ec, &entity, &mut row, &incident, &contact, now)?;
                return Ok(true);
            }
            if status == "Succeeded"
                && field(&row, "verificationStatus").is_empty()
                && Action::parse(&field(&row, "action")).is_some_and(Action::changes_system)
                && millis(row.get("verifyAfterDate")).is_some_and(|at| now >= at)
            {
                verify_action(ec, &entity, &search, &mut row, &incident, &contact, now)?;
                return Ok(true);
            }
            Ok(false)
        })();
        match step {
            Ok(true) => pass.advanced += 1,
            Ok(false) => {}
            Err(error) => {
                tracing::error!(action = %id, %error, "could not move a recovery action on");
                pass.failed.push(format!("action {id}: {error}"));
            }
        }
    }
    let incidents = entity
        .find(INCIDENT)?
        .order_by("detectedDate", true)
        .limit(200)
        .list()?;
    for incident in incidents.values() {
        if !matches!(field(incident, "statusId").as_str(), "Open" | "Reopened" | "Mitigating") {
            continue;
        }
        let id = field(incident, "incidentId");
        let decided = (|| -> Result<bool, Error> {
            let Some((policy_row, policy)) = policy_for(&entity, incident)? else {
                return Ok(false);
            };
            Ok(decide_for(
                ec,
                &entity,
                &search,
                &settings,
                incident,
                &policy_row,
                &policy,
                "system",
                now,
            )?
            .is_some())
        })();
        match decided {
            Ok(true) => pass.decisions += 1,
            Ok(false) => {}
            Err(error) => {
                tracing::error!(incident = %id, %error, "could not decide recovery for an incident");
                pass.failed.push(format!("incident {id}: {error}"));
            }
        }
    }
    Ok(output(
        json!({"decisions": pass.decisions, "advanced": pass.advanced, "failed": pass.failed}),
    ))
}

/// Verify an action whose verify time has come: Verified watches the incident, Failed escalates so the next step of
/// the ladder can be decided, Inconclusive waits for more data until twice the verify time, then escalates.
#[allow(clippy::too_many_arguments)]
fn verify_action(
    ec: &ExecutionContext,
    entity: &EntityFacade,
    search: &SearchFacade,
    row: &mut EntityValue,
    incident: &EntityValue,
    contact: &Json,
    now: i64,
) -> Result<(), Error> {
    let max_rate = policy_for(entity, incident)?.map_or(0.05, |(policy, _)| number(&policy, "maxRollbackRate", 0.05));
    let settings = regression::Settings::read(ec);
    let checked = verify::verify(entity, search, &settings, row, incident, max_rate, now)?;
    let mut record = entity.make_value(VERIFICATION)?;
    record.set("actionId", Value::string(field(row, "actionId")));
    record.set_sequenced_id_secondary(entity)?;
    record.set("verifiedDate", stamp(now));
    record.set("result", Value::string(checked.result.as_str()));
    record.set(
        "checksJson",
        Value::String(Json::Array(checked.checks.clone()).to_string()),
    );
    record.create(entity)?;
    let effective = millis(row.get("effectiveDate")).unwrap_or(now);
    let verify_after = millis(row.get("verifyAfterDate")).unwrap_or(now);
    let waited_enough = now - effective >= 2 * (verify_after - effective).max(MINUTE);
    let final_result = match checked.result {
        verify::Outcome::Inconclusive if !waited_enough => None,
        result => Some(result),
    };
    let Some(result) = final_result else {
        return Ok(());
    };
    row.set("verificationStatus", Value::string(result.as_str()));
    row.update(entity)?;
    let id = field(incident, "incidentId");
    let summary = format!("{} {}: {}", field(row, "action"), field(row, "target"), result.as_str());
    incident::add_evidence(
        entity,
        &id,
        now,
        "verification",
        &field(row, "actionId"),
        &summary,
        Some(&Json::Array(checked.checks)),
    )?;
    match result {
        verify::Outcome::Verified => {
            let mut incident = incident.clone();
            if field(&incident, "statusId") != "Resolved" {
                incident.set("statusId", Value::string("Monitoring"));
                incident.set("recoveredDate", stamp(now));
                incident.set("lastUpdated", stamp(now));
                incident.update(entity)?;
            }
        }
        verify::Outcome::Failed => execute::notify(incident, contact, "Recovery did not work"),
        verify::Outcome::Inconclusive => execute::notify(incident, contact, "Recovery could not be verified"),
    }
    tracing::info!(target: AUDIT, incident = %id, action = %field(row, "actionId"), result = result.as_str(), "recovery verified");
    Ok(())
}

/// An incident's recovery, for `get#Incident`: its decisions newest first, its actions with their verifications, and
/// whether the emergency stop is on.
pub(crate) fn incident_recovery(_ec: &ExecutionContext, entity: &EntityFacade, incident: &str) -> Result<Json, Error> {
    let decisions = entity
        .find(DECISION)?
        .eq("incidentId", Value::string(incident))
        .order_by("decidedDate", true)
        .limit(50)
        .list()?;
    let actions = entity
        .find(execute::ACTION)?
        .eq("incidentId", Value::string(incident))
        .order_by("createdDate", true)
        .limit(50)
        .list()?;
    let mut action_list = Vec::new();
    for row in actions.values() {
        let mut action = row_json(row);
        if let Some(map) = action.as_object_mut() {
            map.remove("callbackTokenHash");
            map.remove("leaseToken");
        }
        let checks = entity
            .find(VERIFICATION)?
            .eq("actionId", Value::string(field(row, "actionId")))
            .order_by("verifiedDate", true)
            .limit(10)
            .list()?;
        action["verifications"] = Json::Array(checks.values().iter().map(row_json).collect());
        action_list.push(action);
    }
    Ok(json!({
        "decisions": decisions.values().iter().map(|row| {
            let mut decision = row_json(row);
            decision["rationale"] = serde_json::from_str(&field(row, "rationaleJson")).unwrap_or(Json::Null);
            decision
        }).collect::<Vec<_>>(),
        "actions": action_list,
        "stopped": stopped(entity)?,
    }))
}

/// `approve#RecoveryDecision`: run a proposed or waiting decision, if the incident still calls for it; a changed
/// incident supersedes it with a new decision instead.
fn approve(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = require("MONITOR_RECOVER")?;
    let options = input(p);
    let id = options["decisionId"].as_str().unwrap_or("");
    let entity = entity_of(ec)?;
    let search = moqui_core::search::current()?;
    let settings = Settings::read(ec);
    let now = now(ec);
    let _authz = ec.disable_authz();
    let mut decision = entity
        .find(DECISION)?
        .eq("decisionId", Value::string(id))
        .one()?
        .ok_or_else(|| fail(format!("no decision {id}")))?;
    if !matches!(field(&decision, "statusId").as_str(), "Proposed" | "AwaitingApproval") {
        return Err(fail(format!(
            "decision {id} is {}, not waiting for approval",
            field(&decision, "statusId")
        )));
    }
    let incident = incident_row(&entity, &field(&decision, "incidentId"))?;
    let Some((policy_row, policy)) = policy_for(&entity, &incident)? else {
        return Err(fail("no recovery policy applies to the incident any more"));
    };
    // The approval is bound to the decision: it must be what the incident calls for now.
    let facts = facts(&entity, &search, &incident, &settings, now)?;
    let outcome = policy::decide(&policy, &facts);
    if decision_hash(&incident, &outcome, &facts) != field(&decision, "decisionHash") {
        decide_for(
            ec,
            &entity,
            &search,
            &settings,
            &incident,
            &policy_row,
            &policy,
            "system",
            now,
        )?;
        return Err(fail(
            "the incident changed since this decision; review the new decision",
        ));
    }
    if facts.stopped {
        return Err(fail("the emergency stop is on"));
    }
    decision.set("statusId", Value::string("Approved"));
    decision.set("approvedByUserId", Value::string(&user));
    decision.set("approvedDate", stamp(now));
    decision.update(&entity)?;
    incident::add_evidence(
        &entity,
        &field(&incident, "incidentId"),
        now,
        "decision",
        id,
        &format!("Approved by {user}"),
        None,
    )?;
    tracing::info!(target: AUDIT, decision = id, user, "recovery decision approved");
    run_decision(ec, &entity, &settings, &decision, &incident, &policy_row, now)?;
    Ok(output(json!({"decision": row_json(&decision)})))
}

/// `refuse#RecoveryDecision`.
fn refuse(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = require("MONITOR_RECOVER")?;
    let options = input(p);
    let id = options["decisionId"].as_str().unwrap_or("");
    let entity = entity_of(ec)?;
    let _authz = ec.disable_authz();
    let mut decision = entity
        .find(DECISION)?
        .eq("decisionId", Value::string(id))
        .one()?
        .ok_or_else(|| fail(format!("no decision {id}")))?;
    if !matches!(field(&decision, "statusId").as_str(), "Proposed" | "AwaitingApproval") {
        return Err(fail(format!(
            "decision {id} is {}, not waiting for approval",
            field(&decision, "statusId")
        )));
    }
    decision.set("statusId", Value::string("Refused"));
    decision.update(&entity)?;
    let reason = options["reason"].as_str().unwrap_or("");
    incident::add_evidence(
        &entity,
        &field(&decision, "incidentId"),
        now(ec),
        "decision",
        id,
        &format!(
            "Refused by {user}{}",
            if reason.is_empty() {
                String::new()
            } else {
                format!(": {}", moqui_core::telemetry::redact_text(reason))
            }
        ),
        None,
    )?;
    tracing::info!(target: AUDIT, decision = id, user, "recovery decision refused");
    Ok(output(json!({"decision": row_json(&decision)})))
}

fn action_row(entity: &EntityFacade, id: &str) -> Result<EntityValue, Error> {
    entity
        .find(execute::ACTION)?
        .eq("actionId", Value::string(id))
        .one()?
        .ok_or_else(|| fail(format!("no recovery action {id}")))
}

/// `complete#RecoveryAction`: an operator marks an action done or failed (a manual action, or a controller that did not
/// report).
fn complete(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = require("MONITOR_RECOVER")?;
    let options = input(p);
    let id = options["actionId"].as_str().unwrap_or("");
    let outcome = match options["outcome"].as_str().unwrap_or("") {
        "done" => "succeeded",
        "failed" => "failed",
        _ => return Err(fail("outcome is done or failed")),
    };
    let entity = entity_of(ec)?;
    let note = format!(
        "marked by {user}{}",
        options["note"]
            .as_str()
            .map(|note| format!(": {}", moqui_core::telemetry::redact_text(note)))
            .unwrap_or_default()
    );
    let report = execute::Report {
        token: None,
        status: outcome,
        external: None,
        message: &note,
    };
    let row = execute::report(ec, &entity, id, &report, now(ec))?;
    tracing::info!(target: AUDIT, action = id, user, outcome, "recovery action completed by hand");
    Ok(output(json!({"action": row_json(&row)})))
}

/// The controller's report on `/v1/recovery`: `{actionId, callbackToken, status, externalId, message}`, which the
/// receiver calls as the ingest key's owner.
pub fn callback(ec: &ExecutionContext, body: &Json) -> Result<Json, Error> {
    let id = body["actionId"].as_str().unwrap_or("");
    let token = body["callbackToken"]
        .as_str()
        .filter(|token| !token.is_empty())
        .ok_or_else(|| fail("callbackToken is required"))?;
    let entity = entity_of(ec)?;
    let message = body["message"].as_str().map(moqui_core::telemetry::redact_text);
    let report = execute::Report {
        token: Some(token),
        status: body["status"].as_str().unwrap_or(""),
        external: body["externalId"].as_str(),
        message: message.as_deref().unwrap_or(""),
    };
    let row = execute::report(ec, &entity, id, &report, now(ec))?;
    tracing::info!(target: AUDIT, action = id, status = %field(&row, "statusId"), "recovery action reported by its controller");
    Ok(json!({"actionId": id, "statusId": field(&row, "statusId")}))
}

/// `cancel#RecoveryAction`.
fn cancel(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = require("MONITOR_RECOVER")?;
    let options = input(p);
    let id = options["actionId"].as_str().unwrap_or("");
    let entity = entity_of(ec)?;
    let _authz = ec.disable_authz();
    let mut row = action_row(&entity, id)?;
    let now = now(ec);
    execute::cancel(ec, &entity, &mut row, now)?;
    incident::add_evidence(
        &entity,
        &field(&row, "incidentId"),
        now,
        "action",
        id,
        &format!("Cancelled by {user}"),
        None,
    )?;
    tracing::info!(target: AUDIT, action = id, user, "recovery action cancelled");
    Ok(output(json!({"action": row_json(&row)})))
}

/// `revert#RecoveryAction`: undo an in-process action.
fn revert(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = require("MONITOR_RECOVER")?;
    let options = input(p);
    let id = options["actionId"].as_str().unwrap_or("");
    let entity = entity_of(ec)?;
    let _authz = ec.disable_authz();
    let mut row = action_row(&entity, id)?;
    if field(&row, "statusId") != "Succeeded" {
        return Err(fail("only an action that took effect can be reverted"));
    }
    let done = execute::revert(ec, &entity, &row)?;
    row.set("statusId", Value::string("Reverted"));
    row.update(&entity)?;
    incident::add_evidence(
        &entity,
        &field(&row, "incidentId"),
        now(ec),
        "action",
        id,
        &format!("Reverted by {user}: {done}"),
        None,
    )?;
    tracing::info!(target: AUDIT, action = id, user, "recovery action reverted");
    Ok(output(json!({"action": row_json(&row)})))
}

/// `set#RecoveryStop`: the emergency stop on or off.
fn set_stop(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = require("MONITOR_RECOVER_ADMIN")?;
    let options = input(p);
    let on = matches!(options["stopped"].as_str(), Some("Y" | "true")) || options["stopped"] == json!(true);
    let entity = entity_of(ec)?;
    let _authz = ec.disable_authz();
    let mut row = entity.make_value(CONTROL)?;
    row.set("controlId", Value::string("RECOVERY"));
    row.set("stopped", Value::string(if on { "Y" } else { "N" }));
    row.set(
        "reason",
        Value::string(moqui_core::telemetry::redact_text(
            options["reason"].as_str().unwrap_or(""),
        )),
    );
    row.set("changedByUserId", Value::string(&user));
    row.set("changedDate", stamp(now(ec)));
    row.create_or_update(&entity)?;
    tracing::warn!(target: AUDIT, user, stopped = on, "recovery emergency stop changed");
    Ok(output(json!({"stopped": on})))
}

/// `list#Recovery`: the policies, the adapters, the newest actions and the emergency stop.
fn list(ec: &ExecutionContext, _: &ServiceInfo<'_>, _: &Parameters) -> Result<Parameters, Error> {
    regression::require_incident()?;
    let entity = entity_of(ec)?;
    let _authz = ec.disable_authz();
    let policies = entity.find(POLICY)?.order_by("sequenceNum", false).list()?;
    let adapters = entity.find(execute::ADAPTER)?.list()?;
    let actions = entity
        .find(execute::ACTION)?
        .order_by("createdDate", true)
        .limit(100)
        .list()?;
    let strip = |row: &EntityValue| {
        let mut value = row_json(row);
        if let Some(map) = value.as_object_mut() {
            map.remove("callbackTokenHash");
            map.remove("leaseToken");
        }
        value
    };
    Ok(output(json!({
        "policies": policies.values().iter().map(row_json).collect::<Vec<_>>(),
        "adapters": adapters.values().iter().map(row_json).collect::<Vec<_>>(),
        "actions": actions.values().iter().map(strip).collect::<Vec<_>>(),
        "stopped": stopped(&entity)?,
    })))
}

/// `store#RecoveryPolicy`: create or update a policy. The ladder must name known actions.
fn store_policy(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = require("MONITOR_RECOVER_ADMIN")?;
    let options = input(p);
    let ladder: Vec<Json> = serde_json::from_str(options["ladderJson"].as_str().unwrap_or("[]"))
        .map_err(|_| fail("ladderJson is a JSON list of steps"))?;
    for step in &ladder {
        if step["action"].as_str().and_then(Action::parse).is_none() {
            return Err(fail(format!("unknown recovery action in {step}")));
        }
    }
    if let Some(pattern) = options["serviceName"].as_str().filter(|text| !text.is_empty()) {
        regex::Regex::new(pattern).map_err(|error| fail(format!("serviceName is not a pattern: {error}")))?;
    }
    if let Some(mode) = options["mode"].as_str() {
        if !matches!(mode, "advisory" | "approval" | "automatic") {
            return Err(fail("mode is advisory, approval or automatic"));
        }
    }
    let entity = entity_of(ec)?;
    let _authz = ec.disable_authz();
    let id = options["policyId"].as_str().filter(|id| !id.is_empty());
    let mut row = match id {
        Some(id) => entity.find(POLICY)?.eq("policyId", Value::string(id)).one()?,
        None => None,
    };
    let fresh = row.is_none();
    let mut value = match row.take() {
        Some(row) => row,
        None => {
            let mut row = entity.make_value(POLICY)?;
            match id {
                Some(id) => row.set("policyId", Value::string(id)),
                None => row.set_sequenced_id_primary(&entity)?,
            }
            row
        }
    };
    for name in [
        "description",
        "enabled",
        "sequenceNum",
        "serviceName",
        "minSeverity",
        "mode",
        "ladderJson",
        "adapterId",
        "contactJson",
        "minConfidence",
        "cooldownMinutes",
        "maxActionsPerDay",
        "deadlineMinutes",
        "verifyMinutes",
        "maxRollbackRate",
    ] {
        if let Some(given) = p.get(name) {
            value.set(name, given.clone());
        }
    }
    if fresh {
        value.create(&entity)?;
    } else {
        value.update(&entity)?;
    }
    tracing::info!(target: AUDIT, policy = %field(&value, "policyId"), user, "recovery policy stored");
    Ok(output(json!({"policyId": field(&value, "policyId")})))
}

/// `store#RecoveryAdapter`: create or update an adapter. A webhook's URL is literal or a property, and its secret and
/// headers only properties, as alert webhooks keep them.
fn store_adapter(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = require("MONITOR_RECOVER_ADMIN")?;
    let options = input(p);
    let id = options["adapterId"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| fail("adapterId is required"))?;
    if policy::AdapterKind::parse(options["adapterType"].as_str().unwrap_or("")).is_none() {
        return Err(fail("adapterType is in-process, webhook or manual"));
    }
    if let Some(url) = options["url"].as_str().filter(|url| !url.is_empty()) {
        if !(url.starts_with("https://") || url.starts_with("http://")) || url.contains('@') {
            return Err(fail(
                "url is an http or https URL without credentials; keep secrets in properties",
            ));
        }
    }
    let entity = entity_of(ec)?;
    let _authz = ec.disable_authz();
    let existing = entity
        .find(execute::ADAPTER)?
        .eq("adapterId", Value::string(id))
        .one()?;
    let fresh = existing.is_none();
    let mut row = match existing {
        Some(row) => row,
        None => {
            let mut row = entity.make_value(execute::ADAPTER)?;
            row.set("adapterId", Value::string(id));
            row
        }
    };
    for name in [
        "description",
        "adapterType",
        "url",
        "urlProperty",
        "statusUrl",
        "secretProperty",
        "headerPropertiesJson",
        "allowInternal",
        "instructions",
    ] {
        if let Some(given) = p.get(name) {
            row.set(name, given.clone());
        }
    }
    if fresh {
        row.create(&entity)?;
    } else {
        row.update(&entity)?;
    }
    tracing::info!(target: AUDIT, adapter = id, user, "recovery adapter stored");
    Ok(output(json!({"adapterId": id})))
}
