//! Verifying a recovery action (plans/Monitoring.md, Later: Recovery; requirement 7). Once the action has been in effect
//! for its verify time, the service since then is compared with each of the incident's regressions' baselines, the
//! business operations' rollback rate is held to the policy's limit, and a rollback must show its instances on the
//! version it went back to. Too few samples or missing telemetry is Inconclusive, never Verified.

use super::{Error, Value};
use crate::regression::{self, compare, field, millis, Window};
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::search::SearchFacade;
use serde_json::{json, Value as Json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Verified,
    Failed,
    Inconclusive,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "Verified",
            Self::Failed => "Failed",
            Self::Inconclusive => "Inconclusive",
        }
    }
}

/// The checks of one action and what they found together.
pub(crate) struct Verification {
    pub result: Outcome,
    pub checks: Vec<Json>,
}

/// Combine check outcomes: any failure fails, then any gap is inconclusive; no checks at all is inconclusive too.
pub fn combine(outcomes: &[Outcome]) -> Outcome {
    if outcomes.contains(&Outcome::Failed) {
        Outcome::Failed
    } else if outcomes.is_empty() || outcomes.contains(&Outcome::Inconclusive) {
        Outcome::Inconclusive
    } else {
        Outcome::Verified
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn verify(
    entity: &EntityFacade,
    search: &SearchFacade,
    settings: &regression::Settings,
    action: &EntityValue,
    incident: &EntityValue,
    max_rollback_rate: f64,
    now: i64,
) -> Result<Verification, Error> {
    let service = field(incident, "serviceName");
    let effective = millis(action.get("effectiveDate")).unwrap_or(now);
    let since = Window {
        service: &service,
        version: None,
        from: effective,
        thru: now,
    };
    let after = regression::stats(search, since)?;
    let mut checks = Vec::new();
    let mut outcomes = Vec::new();
    let regressions = entity
        .find(regression::REGRESSION)?
        .eq("incidentId", Value::string(field(incident, "incidentId")))
        .list()?;
    for row in regressions.values() {
        let Some(metric) = compare::Metric::ALL
            .into_iter()
            .find(|metric| metric.as_str() == field(row, "metric"))
        else {
            continue;
        };
        let baseline = baseline_of(entity, search, settings, row, effective, now)?;
        let verdict = compare::compare(metric, &baseline, &after, &settings.thresholds);
        let outcome = match (&verdict.quality, verdict.regressed) {
            (_, true) => Outcome::Failed,
            (compare::Quality::Good, false) => Outcome::Verified,
            _ => Outcome::Inconclusive,
        };
        outcomes.push(outcome);
        checks.push(json!({
            "check": metric.as_str(),
            "result": outcome.as_str(),
            "baseline": verdict.baseline_value,
            "now": verdict.candidate_value,
            "data": verdict.quality.as_str(),
            "note": verdict.quality.note(),
        }));
    }
    // Business health: the operations' rollback rate since the action.
    if after.operation_runs >= settings.thresholds.min_samples {
        let rate = after.operation_failures as f64 / after.operation_runs as f64;
        let outcome = if rate <= max_rollback_rate {
            Outcome::Verified
        } else {
            Outcome::Failed
        };
        outcomes.push(outcome);
        checks.push(
            json!({"check": "operations", "result": outcome.as_str(), "rollbackRate": rate,
            "limit": max_rollback_rate, "runs": after.operation_runs}),
        );
    } else if after.operation_runs > 0 {
        outcomes.push(Outcome::Inconclusive);
        checks.push(json!({"check": "operations", "result": "Inconclusive", "runs": after.operation_runs,
            "note": format!("{} operation runs; {} are needed", after.operation_runs, settings.thresholds.min_samples)}));
    }
    if field(action, "action") == "rollback" {
        let target = field(action, "target");
        // Only what reported since the rollback took effect: earlier minutes are the version it replaced.
        let running = regression::running_instances(search, &service, effective, now + 60_000)?;
        let outcome = if running.is_empty() {
            Outcome::Inconclusive
        } else if running.iter().all(|(version, _)| *version == target) {
            Outcome::Verified
        } else {
            Outcome::Failed
        };
        let versions: Vec<&str> = running.iter().map(|(version, _)| version.as_str()).collect();
        outcomes.push(outcome);
        checks.push(json!({"check": "version", "result": outcome.as_str(), "expected": target, "running": versions}));
    }
    if after.telemetry_lost > 0 {
        outcomes.push(Outcome::Inconclusive);
        checks.push(json!({"check": "telemetry", "result": "Inconclusive",
            "note": format!("{} telemetry records were dropped since the action", after.telemetry_lost)}));
    }
    Ok(Verification {
        result: combine(&outcomes),
        checks,
    })
}

/// A regression's baseline: the kept one, or for a historical comparison the same window a day before the action.
fn baseline_of(
    entity: &EntityFacade,
    search: &SearchFacade,
    settings: &regression::Settings,
    row: &EntityValue,
    effective: i64,
    now: i64,
) -> Result<compare::Stats, Error> {
    let id = field(row, "baselineId");
    if !id.is_empty() {
        if let Some(baseline) = entity
            .find(regression::BASELINE)?
            .eq("baselineId", Value::string(&id))
            .one()?
        {
            return Ok(serde_json::from_str(&field(&baseline, "statsJson")).unwrap_or_default());
        }
    }
    let service = field(row, "serviceName");
    let version = field(row, "candidateVersion");
    regression::stats(
        search,
        Window {
            service: &service,
            version: Some(&version),
            from: effective - settings.offset,
            thru: now - settings.offset,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_fails_a_gap_is_inconclusive_and_nothing_verifies_nothing() {
        use Outcome::{Failed, Inconclusive, Verified};
        assert_eq!(combine(&[Verified, Verified]), Verified);
        assert_eq!(combine(&[Verified, Inconclusive]), Inconclusive);
        assert_eq!(combine(&[Inconclusive, Failed]), Failed);
        assert_eq!(combine(&[]), Inconclusive);
    }
}
