//! Incidents (plans/Monitoring.md, Later: Regressions and incidents; requirements 5, 7 and 8). An incident opens from
//! a regression at or over `incident-severity`, from an alert rule that fires with the label `incident="true"`, or by
//! hand, one open incident per service: later regressions and alerts join it as evidence. Its timeline holds the
//! regressions, the changes before the onset, new error groups, sample failing traces and missing telemetry, and its
//! ranked causes stay hypotheses until an operator confirms one. When every regression it holds has resolved it goes to
//! Monitoring, and after `resolve-minutes` more to Resolved; a regression back in that time reopens it. Viewing and
//! changing incidents needs `MONITOR_INCIDENT`, and each view and change is logged on the `moqui.monitor.audit` target.

use super::regression::{self, causes, compare, field, millis, now, require_incident, row_json, stamp};
use super::{fail, input, output, Error, ExecutionContext, Parameters, ServiceInfo, Value};
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::search::{Query, SearchFacade, SearchRequest, SortField, Target};
use serde_json::{json, Value as Json};

/// The log target of incident views and changes.
pub const AUDIT: &str = "moqui.monitor.audit";
pub(crate) const INCIDENT: &str = "moqui.monitor.MonitorIncident";
const EVIDENCE: &str = "moqui.monitor.MonitorIncidentEvidence";
const CAUSE: &str = "moqui.monitor.MonitorIncidentCause";
/// The statuses of an incident not yet resolved.
pub const OPEN: [&str; 4] = ["Open", "Mitigating", "Monitoring", "Reopened"];

pub fn register(registry: &mut moqui_core::native::NativeRegistry) -> Result<(), moqui_core::context::Error> {
    for (name, function) in [
        ("list_incidents", list_incidents as moqui_core::native::NativeService),
        ("get_incident", get_incident),
        ("create_incident", create_incident),
        ("update_incident", update_incident),
        ("add_incident_note", add_note),
    ] {
        registry.service_function(&format!("moqui_core::monitor::{name}"), function)?;
    }
    Ok(())
}

/// The open incident of a service, if there is one.
pub(crate) fn open_incident(entity: &EntityFacade, service: &str) -> Result<Option<EntityValue>, Error> {
    let rows = entity
        .find(INCIDENT)?
        .eq("serviceName", Value::string(service))
        .order_by("detectedDate", true)
        .limit(20)
        .list()?;
    Ok(rows
        .values()
        .iter()
        .find(|row| OPEN.contains(&field(row, "statusId").as_str()))
        .cloned())
}

/// Add a row to an incident's timeline.
pub(crate) fn add_evidence(
    entity: &EntityFacade,
    incident: &str,
    at: i64,
    kind: &str,
    reference: &str,
    summary: &str,
    detail: Option<&Json>,
) -> Result<(), Error> {
    let mut row = entity.make_value(EVIDENCE)?;
    row.set("incidentId", Value::string(incident));
    row.set_sequenced_id_secondary(entity)?;
    row.set("evidenceDate", stamp(at));
    row.set("kind", Value::string(kind));
    row.set("refId", Value::string(reference));
    row.set("summary", Value::String(summary.chars().take(4000).collect()));
    if let Some(detail) = detail {
        row.set("detailJson", Value::String(detail.to_string()));
    }
    if let Ok(Some(user)) = moqui_core::user::current_user_id() {
        row.set("userId", Value::String(user));
    }
    row.create(entity)
}

fn percent(value: f64) -> String {
    format!("{:.2}%", value * 100.0)
}

/// How a verdict reads on a timeline.
fn describe(service: &str, version: &str, baseline: &str, verdict: &compare::Verdict) -> String {
    let (from, to) = match verdict.metric {
        compare::Metric::LatencyP95 => (
            format!("{:.3} s", verdict.baseline_value),
            format!("{:.3} s", verdict.candidate_value),
        ),
        _ => (percent(verdict.baseline_value), percent(verdict.candidate_value)),
    };
    format!(
        "{} of {service} {version} rose from {from} to {to} ({:.1}x, confidence {:.1}%) against {baseline}{}",
        verdict.metric.as_str(),
        verdict.magnitude,
        verdict.confidence * 100.0,
        match &verdict.quality {
            compare::Quality::Good => String::new(),
            other => format!("; data {}: {}", other.as_str(), other.note()),
        }
    )
}

/// Open an incident for a regression, or add it to the service's open one. True when an incident was opened.
#[allow(clippy::too_many_arguments)]
pub(crate) fn open_for_regression(
    ec: &ExecutionContext,
    entity: &EntityFacade,
    search: &SearchFacade,
    regression: &EntityValue,
    verdict: &compare::Verdict,
    scope: &causes::Scope,
    ranked: &[causes::Ranked],
    now: i64,
) -> Result<bool, Error> {
    let _authz = ec.disable_authz();
    let service = &scope.service;
    let regression_id = field(regression, "regressionId");
    let baseline = field(regression, "baselineVersion");
    let summary = describe(service, &scope.candidate_version, &baseline, verdict);
    let severity = verdict.severity.map_or("major", compare::Severity::as_str);
    let (mut incident, opened) = match open_incident(entity, service)? {
        Some(incident) => (incident, false),
        None => {
            let resource = moqui_core::telemetry::for_factory(ec.factory()).resource();
            let mut incident = entity.make_value(INCIDENT)?;
            incident.set_sequenced_id_primary(entity)?;
            incident.set(
                "title",
                Value::String(format!(
                    "{service} {}: {} regressed",
                    scope.candidate_version,
                    verdict.metric.as_str()
                )),
            );
            incident.set("severity", Value::string(severity));
            incident.set("statusId", Value::string("Open"));
            incident.set("source", Value::string("regression"));
            incident.set("serviceName", Value::string(service));
            incident.set("version", Value::string(&scope.candidate_version));
            if scope.locality == compare::Locality::Instance {
                incident.set("instanceId", Value::String(scope.affected.join(",")));
            }
            if resource.service_name == *service {
                incident.set("revision", Value::string(&resource.revision));
            }
            incident.set("onsetDate", stamp(scope.onset));
            incident.set("detectedDate", stamp(now));
            incident.set(
                "causeStatus",
                Value::string(if ranked.is_empty() { "unknown" } else { "hypothesis" }),
            );
            incident.set("summary", Value::string(&summary));
            incident.set("lastUpdated", stamp(now));
            incident.create(entity)?;
            (incident, true)
        }
    };
    let incident_id = field(&incident, "incidentId");
    let mut regression = regression.clone();
    regression.set("incidentId", Value::string(&incident_id));
    regression.update(entity)?;
    if !opened {
        // A worse regression raises the incident's severity; one while it was being watched reopens it.
        if severity == "critical" && field(&incident, "severity") != "critical" {
            incident.set("severity", Value::string("critical"));
        }
        if field(&incident, "statusId") == "Monitoring" {
            incident.set("statusId", Value::string("Reopened"));
            incident.set("recoveredDate", Value::Null);
            add_evidence(
                entity,
                &incident_id,
                now,
                "verification",
                "",
                "A regression is back: reopened",
                None,
            )?;
        }
        incident.set("lastUpdated", stamp(now));
        incident.update(entity)?;
    }
    add_evidence(
        entity,
        &incident_id,
        millis(regression.get("firstObserved")).unwrap_or(now),
        "regression",
        &regression_id,
        &summary,
        Some(&json!({
            "baselineInterval": [verdict.baseline_interval.0, verdict.baseline_interval.1],
            "candidateInterval": [verdict.candidate_interval.0, verdict.candidate_interval.1],
            "locality": scope.locality.as_str(),
            "affected": scope.affected,
            "healthy": scope.healthy,
        })),
    )?;
    if let compare::Quality::Degraded(note) = &verdict.quality {
        add_evidence(entity, &incident_id, now, "missing", &regression_id, note, None)?;
    }
    for cause in ranked {
        let Some(at) = cause.time else {
            continue;
        };
        let kind = match cause.kind {
            "release" => "release",
            "error" => "error",
            _ => cause.title.split(':').next().unwrap_or("change"),
        };
        if !has_evidence(entity, &incident_id, kind, &cause.reference)? {
            add_evidence(entity, &incident_id, at, kind, &cause.reference, &cause.title, None)?;
        }
    }
    for (trace, at, text) in failing_traces(search, scope)? {
        if !has_evidence(entity, &incident_id, "trace", &trace)? {
            add_evidence(entity, &incident_id, at, "trace", &trace, &text, None)?;
        }
    }
    if field(&incident, "causeStatus") != "confirmed" {
        save_causes(entity, &incident_id, ranked)?;
        if !ranked.is_empty() && field(&incident, "causeStatus") == "unknown" {
            incident.set("causeStatus", Value::string("hypothesis"));
            incident.update(entity)?;
        }
    }
    tracing::info!(target: AUDIT, incident = %incident_id, regression = %regression_id, opened, "incident from a regression");
    Ok(opened)
}

fn has_evidence(entity: &EntityFacade, incident: &str, kind: &str, reference: &str) -> Result<bool, Error> {
    Ok(entity
        .find(EVIDENCE)?
        .eq("incidentId", Value::string(incident))
        .eq("kind", Value::string(kind))
        .eq("refId", Value::string(reference))
        .limit(1)
        .one()?
        .is_some())
}

/// A few failing requests of the version since the onset: (trace id, start, name and error).
fn failing_traces(search: &SearchFacade, scope: &causes::Scope) -> Result<Vec<(String, i64, String)>, Error> {
    let mut request = SearchRequest::new(Query::all(vec![
        Query::eq("service.name", Value::string(&scope.service)),
        Query::eq("service.version", Value::string(&scope.candidate_version)),
        Query::eq("status_code", Value::string("ERROR")),
        Query::eq("kind", Value::string("SERVER")),
        Query::range("start", Some(Value::I64(scope.onset)), None),
    ]));
    request.sort = vec![SortField {
        field: "start".into(),
        descending: true,
    }];
    request.limit = 5;
    Ok(search
        .search(&Target::Index("otel_spans".into()), &request)?
        .hits
        .iter()
        .filter_map(|hit| {
            let text = |name: &str| {
                hit.body
                    .get(name)
                    .map(moqui_lib::object::to_plain_string)
                    .unwrap_or_default()
            };
            let trace = text("trace_id");
            (!trace.is_empty()).then(|| {
                let error = text("exception.type");
                (
                    trace,
                    millis(hit.body.get("start")).unwrap_or(scope.onset),
                    format!(
                        "{} failed{}",
                        text("name"),
                        if error.is_empty() {
                            String::new()
                        } else {
                            format!(": {error}")
                        }
                    ),
                )
            })
        })
        .collect())
}

/// Replace an incident's hypotheses with `ranked`, keeping the causes an operator confirmed or rejected.
fn save_causes(entity: &EntityFacade, incident: &str, ranked: &[causes::Ranked]) -> Result<(), Error> {
    let existing = entity.find(CAUSE)?.eq("incidentId", Value::string(incident)).list()?;
    let mut decided = Vec::new();
    for row in existing.values() {
        if field(row, "statusId") == "hypothesis" {
            row.delete(entity)?;
        } else {
            decided.push((field(row, "kind"), field(row, "refId")));
        }
    }
    let mut rank = decided.len() as i64;
    for cause in ranked {
        if decided
            .iter()
            .any(|(kind, reference)| kind == cause.kind && *reference == cause.reference)
        {
            continue;
        }
        rank += 1;
        let mut row = entity.make_value(CAUSE)?;
        row.set("incidentId", Value::string(incident));
        row.set_sequenced_id_secondary(entity)?;
        row.set("causeRank", Value::I64(rank));
        row.set("kind", Value::string(cause.kind));
        row.set("refId", Value::string(&cause.reference));
        row.set("title", Value::string(&cause.title));
        row.set("score", Value::F64(cause.score));
        row.set("supportingJson", Value::String(json!(cause.supporting).to_string()));
        row.set(
            "contradictingJson",
            Value::String(json!(cause.contradicting).to_string()),
        );
        row.set("statusId", Value::string("hypothesis"));
        row.create(entity)?;
    }
    Ok(())
}

/// An alert rule labelled `incident="true"` went to Firing: open an incident for its service (the `service` or
/// `service.name` label, else the rule), or add the alert to the open one. A failure is logged: alerting goes on.
pub(crate) fn open_from_alert(ec: &ExecutionContext, rule: &str, title: &str, series: &str, labels: &Json, now: i64) {
    let Some(entity) = ec.factory().entity() else {
        return;
    };
    let opened = entity.transactions_for(INCIDENT).and_then(|tx| {
        tx.run(moqui_core::transaction::TransactionAttribute::ForceNew, None, || {
            let _authz = ec.disable_authz();
            let service = labels["service.name"]
                .as_str()
                .or(labels["service"].as_str())
                .unwrap_or(rule)
                .to_string();
            let incident_id = match open_incident(&entity, &service)? {
                Some(mut incident) => {
                    if field(&incident, "statusId") == "Monitoring" {
                        incident.set("statusId", Value::string("Reopened"));
                    }
                    incident.set("lastUpdated", stamp(now));
                    incident.update(&entity)?;
                    field(&incident, "incidentId")
                }
                None => {
                    let mut incident = entity.make_value(INCIDENT)?;
                    incident.set_sequenced_id_primary(&entity)?;
                    incident.set("title", Value::String(format!("{title} is firing")));
                    incident.set(
                        "severity",
                        Value::string(labels["severity"].as_str().unwrap_or("major")),
                    );
                    incident.set("statusId", Value::string("Open"));
                    incident.set("source", Value::string("alert"));
                    incident.set("serviceName", Value::string(&service));
                    incident.set("onsetDate", stamp(now));
                    incident.set("detectedDate", stamp(now));
                    incident.set("causeStatus", Value::string("unknown"));
                    incident.set("lastUpdated", stamp(now));
                    incident.create(&entity)?;
                    field(&incident, "incidentId")
                }
            };
            add_evidence(
                &entity,
                &incident_id,
                now,
                "alert",
                &format!("{rule}:{series}"),
                &format!("Alert {title} is firing"),
                Some(labels),
            )?;
            Ok(incident_id)
        })
    });
    match opened {
        Ok(incident) => tracing::info!(target: AUDIT, incident, rule, "incident from an alert"),
        Err(error) => tracing::error!(rule, %error, "could not open an incident for a firing alert"),
    }
}

/// After a detection pass: a regression incident whose regressions have all resolved goes to Monitoring, and after
/// `resolve_after` more to Resolved; one with a regression open again while watched is Reopened. Any incident a
/// verified recovery put in Monitoring resolves after the same wait.
pub(crate) fn settle(
    ec: &ExecutionContext,
    entity: &EntityFacade,
    service: &str,
    now: i64,
    resolve_after: i64,
) -> Result<(), Error> {
    let _authz = ec.disable_authz();
    let Some(mut incident) = open_incident(entity, service)? else {
        return Ok(());
    };
    let id = field(&incident, "incidentId");
    let regressions = entity
        .find(regression::REGRESSION)?
        .eq("incidentId", Value::string(&id))
        .list()?;
    let open = regressions.values().iter().any(|row| field(row, "statusId") == "Open");
    let status = field(&incident, "statusId");
    // An alert's or an operator's incident has no regression to go by: only a recovery's verification moves it to
    // Monitoring, and from there it resolves after the same wait.
    if field(&incident, "source") != "regression" && status != "Monitoring" {
        return Ok(());
    }
    let next = match (status.as_str(), open) {
        ("Open" | "Mitigating" | "Reopened", false) => Some("Monitoring"),
        ("Monitoring", true) => Some("Reopened"),
        ("Monitoring", false) if millis(incident.get("recoveredDate")).is_some_and(|at| now - at >= resolve_after) => {
            Some("Resolved")
        }
        _ => None,
    };
    let Some(next) = next else {
        return Ok(());
    };
    incident.set("statusId", Value::string(next));
    match next {
        "Monitoring" => incident.set("recoveredDate", stamp(now)),
        "Resolved" => incident.set("resolvedDate", stamp(now)),
        _ => incident.set("recoveredDate", Value::Null),
    }
    incident.set("lastUpdated", stamp(now));
    incident.update(entity)?;
    let text = match next {
        "Monitoring" => "Every regression has resolved: watching before resolving",
        "Resolved" => "No regression came back: resolved",
        _ => "A regression is back: reopened",
    };
    add_evidence(entity, &id, now, "verification", "", text, None)?;
    tracing::info!(target: AUDIT, incident = %id, status = next, "incident settled");
    Ok(())
}

/// `list#Incidents`: the newest incidents, open ones by default, or of a status or service.
fn list_incidents(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    require_incident()?;
    let options = input(p);
    let entity = ec.factory().entity().ok_or_else(|| fail("no entity facade"))?;
    let _authz = ec.disable_authz();
    let mut find = entity.find(INCIDENT)?;
    if let Some(service) = options["serviceName"].as_str().filter(|value| !value.is_empty()) {
        find = find.eq("serviceName", Value::string(service));
    }
    let status = options["statusId"].as_str().unwrap_or("open");
    let rows = find.order_by("detectedDate", true).limit(500).list()?;
    let incidents: Vec<Json> = rows
        .values()
        .iter()
        .filter(|row| {
            let current = field(row, "statusId");
            match status {
                "open" | "" => OPEN.contains(&current.as_str()),
                "all" => true,
                other => current == other,
            }
        })
        .map(row_json)
        .collect();
    Ok(output(json!({"incidents": incidents})))
}

/// `get#Incident`: the incident, its timeline oldest first, its causes by rank, its regressions, and its recovery.
fn get_incident(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let viewer = require_incident()?;
    let options = input(p);
    let id = options["incidentId"].as_str().unwrap_or("");
    let entity = ec.factory().entity().ok_or_else(|| fail("no entity facade"))?;
    let _authz = ec.disable_authz();
    let incident = entity
        .find(INCIDENT)?
        .eq("incidentId", Value::string(id))
        .one()?
        .ok_or_else(|| fail(format!("no incident {id}")))?;
    tracing::info!(target: AUDIT, incident = id, viewer, "incident viewed");
    let evidence = entity
        .find(EVIDENCE)?
        .eq("incidentId", Value::string(id))
        .order_by("evidenceDate", false)
        .limit(1000)
        .list()?;
    let causes = entity
        .find(CAUSE)?
        .eq("incidentId", Value::string(id))
        .order_by("causeRank", false)
        .list()?;
    let regressions = entity
        .find(regression::REGRESSION)?
        .eq("incidentId", Value::string(id))
        .list()?;
    let causes: Vec<Json> = causes
        .values()
        .iter()
        .map(|row| {
            let mut cause = row_json(row);
            for (from, to) in [("supportingJson", "supporting"), ("contradictingJson", "contradicting")] {
                cause[to] = serde_json::from_str(&field(row, from)).unwrap_or(json!([]));
            }
            cause
        })
        .collect();
    let mut answer = json!({
        "incident": row_json(&incident),
        "evidence": evidence.values().iter().map(row_json).collect::<Vec<_>>(),
        "causes": causes,
        "regressions": regressions.values().iter().map(row_json).collect::<Vec<_>>(),
    });
    answer["recovery"] = super::recovery::incident_recovery(ec, &entity, id)?;
    Ok(output(answer))
}

/// `create#Incident`: an incident opened by hand.
fn create_incident(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = require_incident()?;
    let options = input(p);
    let title = options["title"].as_str().unwrap_or("").trim();
    if title.is_empty() || title.len() > 255 {
        return Err(fail("an incident needs a title of at most 255 characters"));
    }
    let severity = options["severity"].as_str().unwrap_or("major");
    if !matches!(severity, "minor" | "major" | "critical") {
        return Err(fail("severity is minor, major or critical"));
    }
    let entity = ec.factory().entity().ok_or_else(|| fail("no entity facade"))?;
    let now = now(ec);
    let _authz = ec.disable_authz();
    let mut incident = entity.make_value(INCIDENT)?;
    incident.set_sequenced_id_primary(&entity)?;
    incident.set("title", Value::string(title));
    incident.set("severity", Value::string(severity));
    incident.set("statusId", Value::string("Open"));
    incident.set("source", Value::string("user"));
    for name in ["serviceName", "instanceId", "version"] {
        if let Some(value) = options[name].as_str().filter(|value| !value.is_empty()) {
            incident.set(name, Value::string(value));
        }
    }
    incident.set(
        "summary",
        Value::string(moqui_core::telemetry::redact_text(
            options["summary"].as_str().unwrap_or(""),
        )),
    );
    incident.set("onsetDate", stamp(millis(p.get("onsetDate")).unwrap_or(now)));
    incident.set("detectedDate", stamp(now));
    incident.set("causeStatus", Value::string("unknown"));
    incident.set("lastUpdated", stamp(now));
    incident.create(&entity)?;
    let id = field(&incident, "incidentId");
    add_evidence(&entity, &id, now, "note", "", &format!("Opened by {user}"), None)?;
    tracing::info!(target: AUDIT, incident = %id, user, "incident opened by hand");
    Ok(output(json!({"incidentId": id, "incident": row_json(&incident)})))
}

/// `update#Incident`: set the status, title, severity or summary, or confirm or reject a cause.
fn update_incident(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = require_incident()?;
    let options = input(p);
    let id = options["incidentId"].as_str().unwrap_or("");
    let entity = ec.factory().entity().ok_or_else(|| fail("no entity facade"))?;
    let now = now(ec);
    let _authz = ec.disable_authz();
    let mut incident = entity
        .find(INCIDENT)?
        .eq("incidentId", Value::string(id))
        .one()?
        .ok_or_else(|| fail(format!("no incident {id}")))?;
    let mut notes = Vec::new();
    if let Some(status) = options["statusId"].as_str().filter(|value| !value.is_empty()) {
        if !OPEN.contains(&status) && status != "Resolved" {
            return Err(fail("status is Open, Mitigating, Monitoring, Reopened or Resolved"));
        }
        incident.set("statusId", Value::string(status));
        match status {
            "Resolved" => incident.set("resolvedDate", stamp(now)),
            "Monitoring" => incident.set("recoveredDate", stamp(now)),
            _ => {}
        }
        notes.push(format!("Status set to {status} by {user}"));
    }
    if let Some(severity) = options["severity"].as_str().filter(|value| !value.is_empty()) {
        if !matches!(severity, "minor" | "major" | "critical") {
            return Err(fail("severity is minor, major or critical"));
        }
        incident.set("severity", Value::string(severity));
        notes.push(format!("Severity set to {severity} by {user}"));
    }
    for name in ["title", "summary"] {
        if let Some(value) = options[name].as_str() {
            incident.set(name, Value::string(moqui_core::telemetry::redact_text(value)));
        }
    }
    for (parameter, status) in [("confirmCauseSeqId", "confirmed"), ("rejectCauseSeqId", "rejected")] {
        let Some(seq) = options[parameter].as_str().filter(|value| !value.is_empty()) else {
            continue;
        };
        let mut cause = entity
            .find(CAUSE)?
            .eq("incidentId", Value::string(id))
            .eq("causeSeqId", Value::string(seq))
            .one()?
            .ok_or_else(|| fail(format!("no cause {seq} of incident {id}")))?;
        cause.set("statusId", Value::string(status));
        cause.update(&entity)?;
        if status == "confirmed" {
            incident.set("causeStatus", Value::string("confirmed"));
            incident.set("causeConfirmedDate", stamp(now));
        }
        notes.push(format!("Cause {} {status} by {user}", field(&cause, "title")));
    }
    incident.set("lastUpdated", stamp(now));
    incident.update(&entity)?;
    for note in &notes {
        add_evidence(&entity, id, now, "note", "", note, None)?;
    }
    tracing::info!(target: AUDIT, incident = id, user, changes = notes.len(), "incident updated");
    Ok(output(json!({"incident": row_json(&incident)})))
}

/// `add#IncidentNote`: a note on the timeline.
fn add_note(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = require_incident()?;
    let options = input(p);
    let id = options["incidentId"].as_str().unwrap_or("");
    let text = options["text"].as_str().unwrap_or("").trim();
    if text.is_empty() || text.len() > 4000 {
        return Err(fail("a note has 1 to 4000 characters"));
    }
    let entity = ec.factory().entity().ok_or_else(|| fail("no entity facade"))?;
    let _authz = ec.disable_authz();
    if entity
        .find(INCIDENT)?
        .eq("incidentId", Value::string(id))
        .one()?
        .is_none()
    {
        return Err(fail(format!("no incident {id}")));
    }
    add_evidence(
        &entity,
        id,
        now(ec),
        "note",
        "",
        &moqui_core::telemetry::redact_text(text),
        None,
    )?;
    tracing::info!(target: AUDIT, incident = id, user, "incident note added");
    Ok(output(json!({"incidentId": id})))
}
