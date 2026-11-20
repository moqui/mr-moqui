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

//! The fleet screen's rows (plans/Vault-Instances.md §4.8, System › Instances › Fleet): each environment's instances
//! with kind, version, health, seal and link state, cluster, last deploy, open incidents and items past their rotation
//! age, from the rows alone; nothing here reaches a host or an instance.
//!
//! Health is the kind's check (what the host last said: running, and not unhealthy) and how fresh the instance's
//! telemetry is: an mr-moqui instance's ingest key last seen, or its scrape target last read, within
//! [`FRESH_MINUTES`]. `failing` when the check fails, `stale` when it passes and the telemetry is older or missing for
//! an instance that reports, `ok` otherwise, `unknown` before its host was read.

use std::collections::BTreeMap;

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use crate::common::{field, find, one, rows, ACTION, APP_INSTANCE, ENVIRONMENT};
use crate::monitor;

/// How recent an instance's telemetry must be to be fresh.
pub const FRESH_MINUTES: i64 = 10;
/// The statuses of a Monitor incident not yet resolved.
const OPEN_INCIDENT: [&str; 4] = ["Open", "Mitigating", "Monitoring", "Reopened"];

fn millis(row: &EntityValue, name: &str) -> Option<i64> {
    row.get(name).and_then(moqui_core::user::value_millis)
}

/// When an instance's telemetry last came: its ingest keys' last use, or its scrape target's last good read; whether it
/// reports at all.
fn telemetry_of(facade: &EntityFacade, instance_id: &str) -> Result<(bool, Option<i64>), LibError> {
    let mut reports = false;
    let mut last: Option<i64> = None;
    for key in monitor::ingest_keys(facade, instance_id)? {
        if field(&key, "revoked") == "Y" {
            continue;
        }
        reports = true;
        last = last.max(millis(&key, "lastSeen"));
    }
    if let Some(target) = one(
        facade,
        "moqui.monitor.ScrapeTarget",
        "targetId",
        &monitor::scrape_target_id(instance_id),
    )? {
        reports = true;
        if field(&target, "lastStatus") == "ok" {
            last = last.max(millis(&target, "lastScrapeDate"));
        }
    }
    Ok((reports, last))
}

/// The fleet's rows at `now`, by environment then instance name.
pub fn fleet(facade: &EntityFacade, now: i64) -> Result<Vec<BTreeMap<String, Value>>, LibError> {
    let environments: BTreeMap<String, EntityValue> = find(facade, ENVIRONMENT)?
        .list()?
        .into_values()
        .into_iter()
        .map(|row| (field(&row, "environmentId"), row))
        .collect();
    let clusters: BTreeMap<String, EntityValue> = find(facade, "moqui.server.instance.InstanceCluster")?
        .list()?
        .into_values()
        .into_iter()
        .map(|row| (field(&row, "clusterId"), row))
        .collect();
    // each open incident counted for every instance its instanceId names (a regression's names several)
    let mut open_incidents: BTreeMap<String, usize> = BTreeMap::new();
    for incident in find(facade, "moqui.monitor.MonitorIncident")?.list()?.into_values() {
        if OPEN_INCIDENT.contains(&field(&incident, "statusId").as_str()) {
            for id in crate::recovery::split(&field(&incident, "instanceId")) {
                *open_incidents.entry(id).or_default() += 1;
            }
        }
    }
    let mut instances = find(facade, APP_INSTANCE)?.list()?.into_values();
    instances.retain(|row| field(row, "desiredStateEnumId") != "IdsRemoved");
    instances.sort_by_key(|row| (field(row, "environmentId"), field(row, "instanceName")));
    let mut out = Vec::new();
    for row in instances {
        let id = field(&row, "appInstanceId");
        let environment_id = field(&row, "environmentId");
        let environment = environments.get(&environment_id);
        let observed = field(&row, "observedState");
        let check = if observed.is_empty() {
            "unknown"
        } else if observed == "running" && field(&row, "observedHealth") != "unhealthy" {
            "ok"
        } else if field(&row, "desiredStateEnumId") == "IdsRunning" {
            "failing"
        } else {
            // stopped as asked
            "ok"
        };
        let (reports, telemetry_at) = telemetry_of(facade, &id)?;
        let fresh = telemetry_at.is_some_and(|at| now - at <= FRESH_MINUTES * 60_000);
        let telemetry = match (reports, fresh) {
            (false, _) => "none",
            (true, true) => "fresh",
            (true, false) => "stale",
        };
        let health = match check {
            "failing" => "failing",
            "unknown" => "unknown",
            _ if reports && !fresh && field(&row, "desiredStateEnumId") == "IdsRunning" => "stale",
            _ => "ok",
        };
        let deploys: Vec<EntityValue> = rows(facade, ACTION, "appInstanceId", &id)?
            .into_iter()
            .filter(|action| monitor::DEPLOY_ACTIONS.contains(&field(action, "actionEnumId").as_str()))
            .collect();
        let last_deploy = deploys
            .iter()
            .max_by_key(|action| millis(action, "startDate").unwrap_or(0));
        let incidents = open_incidents.get(&id).copied().unwrap_or(0);
        let cluster_id = field(&row, "clusterId");
        let members = field(&row, "clusterMembers");
        let text = |value: String| Value::string(value);
        let mut line = BTreeMap::new();
        for (name, value) in [
            ("environmentId", text(environment_id.clone())),
            (
                "environmentName",
                text(environment.map(|env| field(env, "environmentName")).unwrap_or_default()),
            ),
            ("appInstanceId", text(id.clone())),
            ("instanceName", text(field(&row, "instanceName"))),
            ("instanceKindId", text(field(&row, "instanceKindId"))),
            ("version", text(monitor::version_of(&row))),
            ("imageDigest", text(field(&row, "imageDigest"))),
            ("observedState", text(observed.clone())),
            ("check", text(check.to_string())),
            ("telemetry", text(telemetry.to_string())),
            (
                "telemetryDate",
                telemetry_at.map_or(Value::Null, moqui_core::user::stamp_value),
            ),
            ("health", text(health.to_string())),
            ("sealState", text(field(&row, "sealState"))),
            (
                "sealedSince",
                millis(&row, "sealedSince").map_or(Value::Null, moqui_core::user::stamp_value),
            ),
            ("linkState", text(field(&row, "linkState"))),
            ("clusterId", text(cluster_id.clone())),
            (
                "clusterName",
                text(
                    clusters
                        .get(&cluster_id)
                        .map(|cluster| field(cluster, "clusterName"))
                        .unwrap_or_default(),
                ),
            ),
            (
                "clusterMembers",
                Value::I64(members.split(',').filter(|member| !member.is_empty()).count() as i64),
            ),
            (
                "lastDeployDate",
                last_deploy
                    .and_then(|action| millis(action, "startDate"))
                    .map_or(Value::Null, moqui_core::user::stamp_value),
            ),
            (
                "lastDeployStatus",
                text(
                    last_deploy
                        .map(|action| field(action, "statusEnumId"))
                        .unwrap_or_default(),
                ),
            ),
            ("openIncidents", Value::I64(incidents as i64)),
            // the rotation ages are M5's: until then no item is past its age
            ("itemsPastRotation", Value::Null),
            (
                "rolloutPaused",
                text(
                    if environment.is_some_and(|env| !field(env, "rolloutPausedDate").is_empty()) {
                        "Y"
                    } else {
                        "N"
                    }
                    .to_string(),
                ),
            ),
        ] {
            line.insert(name.to_string(), value);
        }
        out.push(line);
    }
    Ok(out)
}
