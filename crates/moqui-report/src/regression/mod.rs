//! Regression detection (plans/Monitoring.md, Later: Regressions and incidents; requirements 4 and 5). Each pass of
//! `detect#Regressions` compares every service version with request metrics in the last window against its baseline:
//! the previous version's window before the release while the release is new (kept as a `MonitorBaseline`, so it
//! does not change as raw telemetry ages out), and afterwards the same version a day earlier. A regression is kept
//! open while it lasts and resolved when good data shows it gone; one at or over `incident-severity` opens an incident
//! with its evidence and ranked causes ([`super::incident`]). Releases come from the release annotations
//! ([`super::annotations::record_release`]).

pub mod causes;
pub mod compare;

use super::{fail, input, output, Error, ExecutionContext, Parameters, ServiceInfo, Value};
use compare::{Locality, Metric, Quality, Severity, Stats, Thresholds, Verdict};
use indexmap::IndexMap;
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::search::{self, Agg, Query, SearchFacade, SearchRequest, SortField, Target};
use serde_json::{json, Value as Json};

const SETTINGS: &str = "telemetry/regressions";
/// The semantic-convention duration bounds of `http.server.request.duration` and `db.client.operation.duration`.
pub const BOUNDS: [f64; 14] = [
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];
const MINUTE: i64 = 60_000;
const HOUR: i64 = 3_600_000;
pub(crate) const BASELINE: &str = "moqui.monitor.MonitorBaseline";
pub(crate) const REGRESSION: &str = "moqui.monitor.MonitorRegression";
/// The lease the detector holds for a pass.
const DETECTOR: &str = "regressions";

pub fn register(registry: &mut moqui_core::native::NativeRegistry) -> Result<(), moqui_core::context::Error> {
    for (name, function) in [
        ("detect_regressions", detect as moqui_core::native::NativeService),
        ("list_releases", list_releases),
        ("get_release", get_release),
        ("list_regressions", list_regressions),
        ("update_regression", update_regression),
    ] {
        registry.service_function(&format!("moqui_core::monitor::{name}"), function)?;
    }
    Ok(())
}

/// telemetry/regressions.
#[derive(Clone, Debug)]
pub struct Settings {
    pub enabled: bool,
    pub window: i64,
    pub watch: i64,
    pub lookback: i64,
    pub offset: i64,
    pub resolve_after: i64,
    pub incident_severity: Severity,
    pub thresholds: Thresholds,
}

impl Settings {
    pub fn read(ec: &ExecutionContext) -> Self {
        let setting = |name: &str| ec.factory().conf_setting(SETTINGS, name);
        let number = |name: &str, default: f64| {
            setting(name)
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite() && *value >= 0.0)
                .unwrap_or(default)
        };
        let defaults = Thresholds::default();
        Self {
            enabled: setting("enabled") != "false",
            window: (number("window-minutes", 60.0) as i64).max(5) * MINUTE,
            watch: (number("watch-hours", 24.0) * HOUR as f64) as i64,
            lookback: ((number("lookback-hours", 24.0) * HOUR as f64) as i64).max(HOUR),
            offset: ((number("historical-offset-hours", 24.0) * HOUR as f64) as i64).max(HOUR),
            resolve_after: (number("resolve-minutes", 30.0) as i64) * MINUTE,
            incident_severity: if setting("incident-severity") == "critical" {
                Severity::Critical
            } else {
                Severity::Major
            },
            thresholds: Thresholds {
                min_samples: number("min-samples", defaults.min_samples as f64) as u64,
                min_coverage: number("min-coverage", defaults.min_coverage),
                alpha: number("alpha", defaults.alpha),
                rate_delta: number("rate-delta", defaults.rate_delta),
                latency_ratio: number("latency-ratio", defaults.latency_ratio),
                critical_rate: number("critical-rate", defaults.critical_rate),
                critical_ratio: number("critical-ratio", defaults.critical_ratio),
            },
        }
    }
}

pub(crate) fn now(ec: &ExecutionContext) -> i64 {
    ec.effective_time().unwrap_or_else(moqui_core::user::now_millis)
}

pub(crate) fn stamp(millis: i64) -> Value {
    jiff::Timestamp::from_millisecond(millis).map_or(Value::Null, Value::Timestamp)
}

pub(crate) fn millis(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Timestamp(at) => Some(at.as_millisecond()),
        Value::I64(at) => Some(*at),
        other => {
            let text = moqui_lib::object::to_plain_string(other);
            text.parse::<jiff::Timestamp>()
                .map(|at| at.as_millisecond())
                .ok()
                .or_else(|| text.parse::<i64>().ok())
        }
    }
}

pub(crate) fn field(row: &EntityValue, name: &str) -> String {
    row.get(name)
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

fn number_of(value: Option<&Value>) -> f64 {
    value
        .map(moqui_lib::object::to_plain_string)
        .and_then(|text| text.parse::<f64>().ok())
        .unwrap_or(0.0)
}

fn count_of(value: Option<&Value>) -> u64 {
    number_of(value).max(0.0).round() as u64
}

fn aggs(value: Json) -> Result<IndexMap<String, Agg>, Error> {
    Agg::map_from_value(&moqui_lib::json::from_json(&value))
}

/// One service version, or one instance of it, over a window.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Window<'a> {
    pub service: &'a str,
    pub version: Option<&'a str>,
    pub from: i64,
    pub thru: i64,
}

impl Window<'_> {
    fn filter(&self, metric: &str) -> Query {
        let mut all = vec![
            Query::eq("name", Value::string(metric)),
            Query::eq("granularity", Value::string("minute")),
            Query::eq("service.name", Value::string(self.service)),
            Query::range("bucket_start", Some(Value::I64(self.from)), Some(Value::I64(self.thru))),
        ];
        if let Some(version) = self.version {
            all.push(Query::eq("service.version", Value::string(version)));
        }
        Query::all(all)
    }
}

/// The request aggregations a [`Stats`] is read from.
fn request_aggs() -> Json {
    json!({
        "requests": {"sum": {"field": "count"}},
        "latency": {"histogram_merge": {"bounds": BOUNDS}},
        "minutes": {"count_distinct": {"field": "bucket_start"}},
        "errors": {"filter": {"range": {"http.response.status_code": {"gte": 500}}},
                   "aggs": {"count": {"sum": {"field": "count"}}}}
    })
}

fn request_stats(result: &IndexMap<String, Value>, stats: &mut Stats) {
    stats.requests = count_of(result.get("requests"));
    stats.minutes_covered = count_of(result.get("minutes"));
    if let Some(Value::Map(errors)) = result.get("errors") {
        stats.errors = count_of(errors.get("count"));
    }
    stats.bounds = BOUNDS.to_vec();
    stats.buckets = match result.get("latency") {
        Some(Value::Map(latency)) => match latency.get("buckets") {
            Some(Value::List(buckets)) => buckets.iter().map(|count| count_of(Some(count))).collect(),
            _ => vec![0; BOUNDS.len() + 1],
        },
        _ => vec![0; BOUNDS.len() + 1],
    };
}

/// The stats of a window: requests, errors and latency, operation runs and failures, coverage and lost telemetry.
pub(crate) fn stats(search: &SearchFacade, window: Window<'_>) -> Result<Stats, Error> {
    let target = Target::Index("otel_metrics".into());
    let mut stats = Stats {
        minutes_expected: ((window.thru - window.from) / MINUTE).max(1) as u64,
        ..Stats::default()
    };
    let requests = search.aggregate(
        &target,
        &window.filter("http.server.request.duration"),
        &aggs(request_aggs())?,
    )?;
    request_stats(&requests, &mut stats);
    let operations = search.aggregate(
        &target,
        &window.filter("moqui.operation.runs"),
        &aggs(json!({"outcome": {"terms": {"field": "outcome", "size": 10}, "aggs": {"runs": {"sum": {"field": "sum"}}}}}))?,
    )?;
    for bucket in buckets(operations.get("outcome")) {
        let runs = count_of(bucket.get("runs"));
        match bucket.get("key").map(moqui_lib::object::to_plain_string).as_deref() {
            Some("commit") => stats.operation_runs += runs,
            Some("rollback" | "retry") => {
                stats.operation_runs += runs;
                stats.operation_failures += runs;
            }
            _ => {}
        }
    }
    // The dropped counter is a running total per process: what it gained over the window.
    let lost = search.aggregate(
        &target,
        &window.filter("moqui.telemetry.dropped"),
        &aggs(json!({"high": {"max": {"field": "last"}}, "low": {"min": {"field": "last"}}}))?,
    )?;
    stats.telemetry_lost = (number_of(lost.get("high")) - number_of(lost.get("low"))).max(0.0) as u64;
    Ok(stats)
}

/// The request stats of each instance of a version over a window.
fn instance_stats(search: &SearchFacade, window: Window<'_>) -> Result<Vec<(String, Stats)>, Error> {
    let result = search.aggregate(
        &Target::Index("otel_metrics".into()),
        &window.filter("http.server.request.duration"),
        &aggs(json!({"instance": {"terms": {"field": "service.instance.id", "size": 50}, "aggs": request_aggs()}}))?,
    )?;
    let expected = ((window.thru - window.from) / MINUTE).max(1) as u64;
    Ok(buckets(result.get("instance"))
        .iter()
        .filter_map(|bucket| {
            let instance = moqui_lib::object::to_plain_string(bucket.get("key")?);
            let mut stats = Stats {
                minutes_expected: expected,
                ..Stats::default()
            };
            request_stats(bucket, &mut stats);
            Some((instance, stats))
        })
        .collect())
}

fn buckets(value: Option<&Value>) -> Vec<IndexMap<String, Value>> {
    let Some(Value::Map(map)) = value else {
        return Vec::new();
    };
    let Some(Value::List(list)) = map.get("buckets") else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|bucket| match bucket {
            Value::Map(bucket) => Some(bucket.clone()),
            _ => None,
        })
        .collect()
}

/// A version's first release marker.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Release {
    pub version: String,
    pub revision: String,
    pub time: i64,
    pub instances: Vec<String>,
    pub uncertain: bool,
    /// The release's entity schema fingerprint ([`super::annotations::schema_hash`]), when it recorded one.
    pub schema: Option<String>,
}

/// The releases of a service, oldest first, each version once at its first marker.
pub(crate) fn releases(search: &SearchFacade, service: &str) -> Result<Vec<Release>, Error> {
    let mut request = SearchRequest::new(Query::all(vec![
        Query::eq("source", Value::string("release")),
        Query::eq("service.name", Value::string(service)),
    ]));
    request.sort = vec![SortField {
        field: "time".into(),
        descending: false,
    }];
    request.limit = 1000;
    let page = search.search(&Target::Index("monitor_annotations".into()), &request)?;
    let mut out: Vec<Release> = Vec::new();
    for hit in &page.hits {
        let text = |name: &str| {
            hit.body
                .get(name)
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_default()
        };
        let version = text("service.version");
        let Some(time) = millis(hit.body.get("time")) else {
            continue;
        };
        let instance = text("service.instance.id");
        let uncertain = text("attribution_uncertain") == "true";
        match out.iter_mut().find(|release| release.version == version) {
            Some(release) => {
                if !instance.is_empty() && !release.instances.contains(&instance) {
                    release.instances.push(instance);
                }
            }
            None => out.push(Release {
                version,
                revision: text("vcs.ref.head.revision"),
                time,
                instances: if instance.is_empty() {
                    Vec::new()
                } else {
                    vec![instance]
                },
                uncertain,
                schema: Some(text("moqui.schema.hash")).filter(|hash| !hash.is_empty()),
            }),
        }
    }
    out.sort_by_key(|release| release.time);
    Ok(out)
}

/// The services and versions with request metrics in a window.
fn active_versions(search: &SearchFacade, from: i64, thru: i64) -> Result<Vec<(String, String)>, Error> {
    let result = search.aggregate(
        &Target::Index("otel_metrics".into()),
        &Query::all(vec![
            Query::eq("name", Value::string("http.server.request.duration")),
            Query::eq("granularity", Value::string("minute")),
            Query::range("bucket_start", Some(Value::I64(from)), Some(Value::I64(thru))),
        ]),
        &aggs(json!({"service": {"terms": {"field": "service.name", "size": 100},
            "aggs": {"version": {"terms": {"field": "service.version", "size": 20}}}}}))?,
    )?;
    let mut out = Vec::new();
    for service in buckets(result.get("service")) {
        let name = service
            .get("key")
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default();
        for version in buckets(service.get("version")) {
            let version = version
                .get("key")
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_default();
            if !name.is_empty() && !version.is_empty() {
                out.push((name.clone(), version));
            }
        }
    }
    Ok(out)
}

/// What a version is compared with.
#[derive(Clone, Debug)]
pub(crate) struct Baseline {
    pub id: Option<String>,
    /// `version` or `historical`.
    pub kind: &'static str,
    pub version: String,
    pub stats: Stats,
    /// The release that made this a version comparison, and the window the candidate is read over.
    pub release: Option<Release>,
    pub candidate_from: i64,
}

/// The baseline of a version now: its release's previous version while the release is within `watch`, else the same
/// version `offset` earlier.
fn baseline(
    ec: &ExecutionContext,
    entity: &EntityFacade,
    search: &SearchFacade,
    settings: &Settings,
    service: &str,
    version: &str,
    now: i64,
) -> Result<Baseline, Error> {
    let releases = releases(search, service)?;
    let position = releases.iter().position(|release| release.version == version);
    if let Some(index) = position.filter(|index| *index > 0) {
        let release = &releases[index];
        if now - release.time <= settings.watch {
            let previous = &releases[index - 1];
            let (id, stats) = stored_baseline(ec, entity, search, settings, service, &previous.version, release.time)?;
            return Ok(Baseline {
                id: Some(id),
                kind: "version",
                version: previous.version.clone(),
                stats,
                release: Some(release.clone()),
                candidate_from: release.time.max(now - settings.window),
            });
        }
    }
    let window = Window {
        service,
        version: Some(version),
        from: now - settings.offset - settings.window,
        thru: now - settings.offset,
    };
    Ok(Baseline {
        id: None,
        kind: "historical",
        version: version.to_string(),
        stats: stats(search, window)?,
        release: None,
        candidate_from: now - settings.window,
    })
}

/// The previous version's stats over the lookback before a release, read once and kept.
fn stored_baseline(
    ec: &ExecutionContext,
    entity: &EntityFacade,
    search: &SearchFacade,
    settings: &Settings,
    service: &str,
    version: &str,
    release_time: i64,
) -> Result<(String, Stats), Error> {
    let _authz = ec.disable_authz();
    let existing = entity
        .find(BASELINE)?
        .eq("serviceName", Value::string(service))
        .eq("version", Value::string(version))
        .eq("baselineKind", Value::string("version"))
        .eq("thruDate", stamp(release_time))
        .one()?;
    if let Some(row) = existing {
        let stats = serde_json::from_str(&field(&row, "statsJson")).unwrap_or_default();
        return Ok((field(&row, "baselineId"), stats));
    }
    let from = release_time - settings.lookback;
    let stats = stats(
        search,
        Window {
            service,
            version: Some(version),
            from,
            thru: release_time,
        },
    )?;
    let mut row = entity.make_value(BASELINE)?;
    row.set_sequenced_id_primary(entity)?;
    row.set("serviceName", Value::string(service));
    row.set("version", Value::string(version));
    row.set("baselineKind", Value::string("version"));
    row.set("fromDate", stamp(from));
    row.set("thruDate", stamp(release_time));
    row.set("sampleCount", Value::I64(stats.requests as i64));
    row.set("statsJson", Value::String(serde_json::to_string(&stats).map_err(fail)?));
    row.set("computedDate", stamp(now(ec)));
    row.create(entity)?;
    Ok((field(&row, "baselineId"), stats))
}

/// What one pass did.
#[derive(Default)]
struct Pass {
    compared: usize,
    regressions: usize,
    resolved: usize,
    incidents: usize,
    failed: Vec<String>,
}

/// `detect#Regressions`: one pass over every service version with traffic, under the detector's lease so two servers
/// do not both run it. A version that cannot be compared is logged and the pass goes on.
fn detect(ec: &ExecutionContext, _: &ServiceInfo<'_>, _: &Parameters) -> Result<Parameters, Error> {
    let settings = Settings::read(ec);
    if !settings.enabled {
        return Ok(output(json!({"skipped": "disabled"})));
    }
    let entity = ec.factory().entity().ok_or_else(|| fail("no entity facade"))?;
    let search = search::current()?;
    let Some(lease) = super::lock::claim(ec, &entity, DETECTOR, 10 * MINUTE)? else {
        return Ok(output(json!({"skipped": "another server is detecting"})));
    };
    let now = now(ec);
    let mut pass = Pass::default();
    // The lease goes back however the pass ends, so a failed pass does not hold the next one off for its whole term.
    let versions = active_versions(&search, now - settings.window, now);
    let versions = match versions {
        Ok(versions) => versions,
        Err(error) => {
            super::lock::release(ec, &entity, DETECTOR, &lease);
            return Err(error);
        }
    };
    for (service, version) in &versions {
        match compare_version(ec, &entity, &search, &settings, service, version, now, &mut pass) {
            Ok(()) => pass.compared += 1,
            Err(error) => {
                tracing::error!(service, version, %error, "could not compare a service version for regressions");
                pass.failed.push(format!("{service} {version}: {error}"));
            }
        }
    }
    let services: std::collections::BTreeSet<&str> = versions.iter().map(|(service, _)| service.as_str()).collect();
    for service in services {
        if let Err(error) = super::incident::settle(ec, &entity, service, now, settings.resolve_after) {
            tracing::error!(service, %error, "could not settle the incidents of a service");
            pass.failed.push(format!("{service}: {error}"));
        }
    }
    super::lock::release(ec, &entity, DETECTOR, &lease);
    Ok(output(json!({
        "compared": pass.compared,
        "regressions": pass.regressions,
        "resolved": pass.resolved,
        "incidents": pass.incidents,
        "failed": pass.failed,
    })))
}

#[allow(clippy::too_many_arguments)]
fn compare_version(
    ec: &ExecutionContext,
    entity: &EntityFacade,
    search: &SearchFacade,
    settings: &Settings,
    service: &str,
    version: &str,
    now: i64,
    pass: &mut Pass,
) -> Result<(), Error> {
    let baseline = baseline(ec, entity, search, settings, service, version, now)?;
    let window = Window {
        service,
        version: Some(version),
        from: baseline.candidate_from,
        thru: now,
    };
    let candidate = stats(search, window)?;
    for metric in Metric::ALL {
        let verdict = compare::compare(metric, &baseline.stats, &candidate, &settings.thresholds);
        let open = open_regression(ec, entity, service, metric, version)?;
        if verdict.regressed {
            let (locality, affected, healthy) = locality(search, &baseline, window, metric, &settings.thresholds)?;
            let fresh = open.is_none();
            let row = save_regression(
                ec,
                entity,
                open,
                service,
                version,
                &baseline,
                &verdict,
                locality,
                &affected,
                &healthy,
                window.from,
                now,
            )?;
            if fresh {
                pass.regressions += 1;
            }
            if verdict
                .severity
                .is_some_and(|severity| severity >= settings.incident_severity)
                && field(&row, "incidentId").is_empty()
            {
                let scope = causes::Scope {
                    service: service.to_string(),
                    candidate_version: version.to_string(),
                    baseline_version: (baseline.kind == "version").then(|| baseline.version.clone()),
                    onset: window.from,
                    locality,
                    affected,
                    healthy,
                    errors: candidate.errors,
                    old_version_regressed: old_version_regressed(search, &baseline, window, metric, settings)?,
                };
                let candidates = cause_candidates(search, settings, &scope, &baseline, window)?;
                let ranked = causes::rank(&scope, &candidates, settings.lookback);
                if super::incident::open_for_regression(ec, entity, search, &row, &verdict, &scope, &ranked, now)? {
                    pass.incidents += 1;
                }
            }
        } else if let Some(mut open) = open {
            // Only good data says a regression is gone; gaps or too few samples leave it open.
            if verdict.quality == Quality::Good {
                let _authz = ec.disable_authz();
                open.set("statusId", Value::string("Resolved"));
                open.set("resolvedDate", stamp(now));
                open.set("candidateValue", Value::F64(verdict.candidate_value));
                open.update(entity)?;
                pass.resolved += 1;
            }
        }
    }
    Ok(())
}

fn open_regression(
    ec: &ExecutionContext,
    entity: &EntityFacade,
    service: &str,
    metric: Metric,
    version: &str,
) -> Result<Option<EntityValue>, Error> {
    let _authz = ec.disable_authz();
    entity
        .find(REGRESSION)?
        .eq("serviceName", Value::string(service))
        .eq("metric", Value::string(metric.as_str()))
        .eq("candidateVersion", Value::string(version))
        .eq("statusId", Value::string("Open"))
        .one()
}

#[allow(clippy::too_many_arguments)]
fn save_regression(
    ec: &ExecutionContext,
    entity: &EntityFacade,
    open: Option<EntityValue>,
    service: &str,
    version: &str,
    baseline: &Baseline,
    verdict: &Verdict,
    locality: Locality,
    affected: &[String],
    healthy: &[String],
    onset: i64,
    now: i64,
) -> Result<EntityValue, Error> {
    let _authz = ec.disable_authz();
    let fresh = open.is_none();
    let mut row = match open {
        Some(row) => row,
        None => {
            let mut row = entity.make_value(REGRESSION)?;
            row.set_sequenced_id_primary(entity)?;
            row.set("serviceName", Value::string(service));
            row.set("metric", Value::string(verdict.metric.as_str()));
            row.set("candidateVersion", Value::string(version));
            row.set("statusId", Value::string("Open"));
            row.set("firstObserved", stamp(onset));
            row
        }
    };
    row.set("baselineId", baseline.id.clone().map_or(Value::Null, Value::String));
    row.set("baselineVersion", Value::string(&baseline.version));
    row.set("baselineValue", Value::F64(verdict.baseline_value));
    row.set("candidateValue", Value::F64(verdict.candidate_value));
    row.set("magnitude", Value::F64(verdict.magnitude));
    row.set("confidence", Value::F64(verdict.confidence));
    row.set("baselineSamples", Value::I64(verdict.baseline_samples as i64));
    row.set("candidateSamples", Value::I64(verdict.candidate_samples as i64));
    row.set(
        "severity",
        Value::string(verdict.severity.map_or("major", Severity::as_str)),
    );
    row.set("dataQuality", Value::string(verdict.quality.as_str()));
    row.set("qualityNote", Value::string(verdict.quality.note()));
    row.set("locality", Value::string(locality.as_str()));
    row.set("affectedInstances", Value::String(affected.join(",")));
    row.set("healthyInstances", Value::String(healthy.join(",")));
    row.set("lastObserved", stamp(now));
    if fresh {
        row.create(entity)?;
    } else {
        row.update(entity)?;
    }
    Ok(row)
}

/// Each candidate instance compared with the baseline on the metric that regressed.
fn locality(
    search: &SearchFacade,
    baseline: &Baseline,
    window: Window<'_>,
    metric: Metric,
    thresholds: &Thresholds,
) -> Result<(Locality, Vec<String>, Vec<String>), Error> {
    if metric == Metric::RollbackRate {
        // Operation runs are not split by instance here.
        return Ok((Locality::Unknown, Vec::new(), Vec::new()));
    }
    let verdicts: Vec<(String, Verdict)> = instance_stats(search, window)?
        .into_iter()
        .map(|(instance, stats)| {
            let verdict = compare::compare(metric, &baseline.stats, &stats, thresholds);
            (instance, verdict)
        })
        .collect();
    Ok(compare::locality(&verdicts))
}

/// Whether instances still on the previous version regressed over the candidate's window: if they did, the release
/// is not what changed.
fn old_version_regressed(
    search: &SearchFacade,
    baseline: &Baseline,
    window: Window<'_>,
    metric: Metric,
    settings: &Settings,
) -> Result<Option<bool>, Error> {
    if baseline.kind != "version" {
        return Ok(None);
    }
    let old = stats(
        search,
        Window {
            version: Some(&baseline.version),
            ..window
        },
    )?;
    let verdict = compare::compare(metric, &baseline.stats, &old, &settings.thresholds);
    Ok(match verdict.quality {
        Quality::Insufficient(_) => None,
        _ => Some(verdict.regressed),
    })
}

/// The causes worth ranking: the release, deploy and config changes before the onset, the instances, slower
/// dependencies and error groups new near the onset.
fn cause_candidates(
    search: &SearchFacade,
    settings: &Settings,
    scope: &causes::Scope,
    baseline: &Baseline,
    window: Window<'_>,
) -> Result<Vec<causes::Candidate>, Error> {
    let mut candidates = Vec::new();
    if let Some(release) = &baseline.release {
        candidates.push(causes::Candidate::Release {
            version: release.version.clone(),
            revision: release.revision.clone(),
            time: release.time,
        });
    }
    let mut request = SearchRequest::new(Query::all(vec![
        Query::is_in("source", vec![Value::string("deploy"), Value::string("config")]),
        Query::range(
            "time",
            Some(Value::I64(scope.onset - settings.lookback)),
            Some(Value::I64(window.thru)),
        ),
        Query::any(vec![
            Query::eq("service.name", Value::string(&scope.service)),
            Query::exclude(Query::exists("service.name")),
        ]),
    ]));
    request.limit = 50;
    for hit in search
        .search(&Target::Index("monitor_annotations".into()), &request)?
        .hits
    {
        let text = |name: &str| {
            hit.body
                .get(name)
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_default()
        };
        let Some(time) = millis(hit.body.get("time")) else {
            continue;
        };
        let instance = text("service.instance.id");
        candidates.push(causes::Candidate::Change {
            id: hit.id.clone(),
            source: text("source"),
            time,
            instance: (!instance.is_empty()).then_some(instance),
            text: text("text"),
        });
    }
    for instance in &scope.affected {
        candidates.push(causes::Candidate::Instance { id: instance.clone() });
    }
    candidates.extend(slower_dependencies(search, baseline, window)?);
    let mut request = SearchRequest::new(Query::all(vec![
        Query::eq("service.name", Value::string(&scope.service)),
        Query::range("first_seen", Some(Value::I64(scope.onset - settings.lookback)), None),
    ]));
    request.sort = vec![SortField {
        field: "count".into(),
        descending: true,
    }];
    request.limit = 10;
    for hit in search.search(&Target::Index("otel_errors".into()), &request)?.hits {
        let Some(first_seen) = millis(hit.body.get("first_seen")) else {
            continue;
        };
        candidates.push(causes::Candidate::Error {
            fingerprint: hit
                .body
                .get("fingerprint")
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_else(|| hit.id.clone()),
            kind: hit
                .body
                .get("exception.type")
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_default(),
            first_seen,
            count: count_of(hit.body.get("count")),
        });
    }
    Ok(candidates)
}

/// Each database system whose statement p95 in the candidate window is over its p95 in the baseline's window.
fn slower_dependencies(
    search: &SearchFacade,
    baseline: &Baseline,
    window: Window<'_>,
) -> Result<Vec<causes::Candidate>, Error> {
    let p95_by_system = |window: Window<'_>| -> Result<Vec<(String, f64)>, Error> {
        let result = search.aggregate(
            &Target::Index("otel_metrics".into()),
            &window.filter("db.client.operation.duration"),
            &aggs(json!({"system": {"terms": {"field": "db.system.name", "size": 10},
                "aggs": {"latency": {"histogram_merge": {"bounds": BOUNDS}}}}}))?,
        )?;
        Ok(buckets(result.get("system"))
            .iter()
            .filter_map(|bucket| {
                let name = moqui_lib::object::to_plain_string(bucket.get("key")?);
                let Some(Value::Map(latency)) = bucket.get("latency") else {
                    return None;
                };
                let Some(Value::List(counts)) = latency.get("buckets") else {
                    return None;
                };
                let counts: Vec<u64> = counts.iter().map(|count| count_of(Some(count))).collect();
                let p95 = moqui_core::telemetry::maintenance::histogram_percentile(&BOUNDS, &counts, 0.95)?;
                Some((name, p95))
            })
            .collect())
    };
    let length = window.thru - window.from;
    let before = match &baseline.release {
        Some(release) => Window {
            version: Some(&baseline.version),
            from: release.time - length,
            thru: release.time,
            ..window
        },
        None => Window {
            from: window.from - length,
            thru: window.from,
            ..window
        },
    };
    let earlier = p95_by_system(before)?;
    Ok(p95_by_system(window)?
        .into_iter()
        .filter_map(|(name, now)| {
            let (_, was) = earlier.iter().find(|(system, _)| *system == name)?;
            (*was > 0.0 && now / was > 1.5).then(|| causes::Candidate::Dependency { name, ratio: now / was })
        })
        .collect())
}

pub(crate) fn require_incident() -> Result<String, Error> {
    let user = moqui_core::user::current_user_id()
        .map_err(fail)?
        .ok_or_else(|| fail("sign in to see incidents"))?;
    if moqui_core::user::has_permission("MONITOR_INCIDENT").map_err(fail)? {
        Ok(user)
    } else {
        Err(fail(
            "MONITOR_INCIDENT permission is required to see regressions and incidents",
        ))
    }
}

pub(crate) fn row_json(row: &EntityValue) -> Json {
    let map: serde_json::Map<String, Json> = row
        .fields()
        .iter()
        .map(|(name, value)| (name.to_string(), moqui_lib::json::to_json(value)))
        .collect();
    Json::Object(map)
}

/// `list#Releases`: each service's versions, newest first, with the instances that reported each in the last 15
/// minutes and the regressions found for it.
fn list_releases(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    require_incident()?;
    let options = input(p);
    let search = search::current()?;
    let entity = ec.factory().entity().ok_or_else(|| fail("no entity facade"))?;
    let services: Vec<String> = match options["serviceName"].as_str().filter(|name| !name.is_empty()) {
        Some(service) => vec![service.to_string()],
        None => {
            let result = search.aggregate(
                &Target::Index("monitor_annotations".into()),
                &Query::eq("source", Value::string("release")),
                &aggs(json!({"service": {"terms": {"field": "service.name", "size": 100}}}))?,
            )?;
            buckets(result.get("service"))
                .iter()
                .filter_map(|bucket| bucket.get("key").map(moqui_lib::object::to_plain_string))
                .collect()
        }
    };
    let now = now(ec);
    let mut out = Vec::new();
    for service in services {
        let running = running_instances(&search, &service, now - 15 * MINUTE, now + MINUTE)?;
        let releases = releases(&search, &service)?;
        for (index, release) in releases.iter().enumerate().rev() {
            let regressions = regressions_of(ec, &entity, &service, &release.version)?;
            out.push(json!({
                "serviceName": service,
                "version": release.version,
                "revision": release.revision,
                "releasedDate": jiff::Timestamp::from_millisecond(release.time).map(|at| at.to_string()).unwrap_or_default(),
                "previousVersion": index.checked_sub(1).map(|previous| releases[previous].version.clone()),
                "attributionUncertain": release.uncertain,
                "instancesNow": running.iter().filter(|(version, _)| *version == release.version).map(|(_, instance)| instance.clone()).collect::<Vec<_>>(),
                "openRegressions": regressions.iter().filter(|row| row["statusId"] == "Open").count(),
                "regressions": regressions.len(),
            }));
        }
    }
    Ok(output(json!({"releases": out})))
}

/// The (version, instance) pairs with request metrics from `from` until `thru`.
pub(crate) fn running_instances(
    search: &SearchFacade,
    service: &str,
    from: i64,
    thru: i64,
) -> Result<Vec<(String, String)>, Error> {
    let result = search.aggregate(
        &Target::Index("otel_metrics".into()),
        &Window {
            service,
            version: None,
            from,
            thru,
        }
        .filter("http.server.request.duration"),
        &aggs(json!({"version": {"terms": {"field": "service.version", "size": 20},
            "aggs": {"instance": {"terms": {"field": "service.instance.id", "size": 50}}}}}))?,
    )?;
    let mut out = Vec::new();
    for version in buckets(result.get("version")) {
        let name = version
            .get("key")
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default();
        for instance in buckets(version.get("instance")) {
            if let Some(id) = instance.get("key") {
                out.push((name.clone(), moqui_lib::object::to_plain_string(id)));
            }
        }
    }
    Ok(out)
}

fn regressions_of(
    ec: &ExecutionContext,
    entity: &EntityFacade,
    service: &str,
    version: &str,
) -> Result<Vec<Json>, Error> {
    let _authz = ec.disable_authz();
    let rows = entity
        .find(REGRESSION)?
        .eq("serviceName", Value::string(service))
        .eq("candidateVersion", Value::string(version))
        .order_by("lastObserved", true)
        .limit(200)
        .list()?;
    Ok(rows.values().iter().map(row_json).collect())
}

/// `get#Release`: a version's release, the version before it, its regressions and the error groups first seen since.
fn get_release(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let viewer = require_incident()?;
    let options = input(p);
    let service = options["serviceName"].as_str().unwrap_or("");
    let version = options["version"].as_str().unwrap_or("");
    let search = search::current()?;
    let entity = ec.factory().entity().ok_or_else(|| fail("no entity facade"))?;
    let releases = releases(&search, service)?;
    let Some(index) = releases.iter().position(|release| release.version == version) else {
        return Err(fail(format!("no release of {service} {version}")));
    };
    let release = &releases[index];
    tracing::info!(target: super::incident::AUDIT, service, version, viewer, "release viewed");
    let mut request = SearchRequest::new(Query::all(vec![
        Query::eq("service.name", Value::string(service)),
        Query::range("first_seen", Some(Value::I64(release.time)), None),
    ]));
    request.sort = vec![SortField {
        field: "count".into(),
        descending: true,
    }];
    request.limit = 20;
    let errors: Vec<Json> = search
        .search(&Target::Index("otel_errors".into()), &request)?
        .hits
        .iter()
        .map(|hit| {
            let text = |name: &str| hit.body.get(name).map(moqui_lib::json::to_json).unwrap_or(Json::Null);
            json!({"fingerprint": text("fingerprint"), "type": text("exception.type"), "message": text("message"),
                "firstSeen": text("first_seen"), "count": text("count")})
        })
        .collect();
    Ok(output(json!({
        "release": {
            "serviceName": service,
            "version": release.version,
            "revision": release.revision,
            "releasedDate": jiff::Timestamp::from_millisecond(release.time).map(|at| at.to_string()).unwrap_or_default(),
            "instances": release.instances,
            "attributionUncertain": release.uncertain,
            "previousVersion": index.checked_sub(1).map(|previous| releases[previous].version.clone()),
        },
        "regressions": regressions_of(ec, &entity, service, version)?,
        "newErrors": errors,
    })))
}

/// `list#Regressions`: the newest regressions, of a service or status when given.
fn list_regressions(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    require_incident()?;
    let options = input(p);
    let entity = ec.factory().entity().ok_or_else(|| fail("no entity facade"))?;
    let _authz = ec.disable_authz();
    let mut find = entity.find(REGRESSION)?;
    for (parameter, name) in [
        ("serviceName", "serviceName"),
        ("statusId", "statusId"),
        ("incidentId", "incidentId"),
    ] {
        if let Some(value) = options[parameter].as_str().filter(|value| !value.is_empty()) {
            find = find.eq(name, Value::string(value));
        }
    }
    let rows = find.order_by("lastObserved", true).limit(200).list()?;
    Ok(output(
        json!({"regressions": rows.values().iter().map(row_json).collect::<Vec<_>>()}),
    ))
}

/// `update#Regression`: dismiss a regression (not a regression after all), or open it again.
fn update_regression(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = require_incident()?;
    let options = input(p);
    let id = options["regressionId"].as_str().unwrap_or("");
    let status = options["statusId"].as_str().unwrap_or("");
    if !matches!(status, "Dismissed" | "Open") {
        return Err(fail("a regression can be set to Dismissed or Open"));
    }
    let entity = ec.factory().entity().ok_or_else(|| fail("no entity facade"))?;
    let _authz = ec.disable_authz();
    let mut row = entity
        .find(REGRESSION)?
        .eq("regressionId", Value::string(id))
        .one()?
        .ok_or_else(|| fail(format!("no regression {id}")))?;
    row.set("statusId", Value::string(status));
    row.update(&entity)?;
    tracing::info!(target: super::incident::AUDIT, regression = id, status, user, "regression updated");
    Ok(output(json!({"regression": row_json(&row)})))
}
