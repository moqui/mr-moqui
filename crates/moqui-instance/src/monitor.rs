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

//! Monitor for managed instances (plans/Vault-Instances.md §4.8), the manager's side:
//!
//! - **Identity.** A provision gives a new mr-moqui instance its `instance@id` (so its `service.instance.id` is its
//!   `AppInstance` id), its `telemetry@deployment-environment` (the environment's id), its `telemetry@service-name`
//!   (`moqui-{environment}`) and its OTLP export to this
//!   manager's Monitor (`manage@monitor-url`, else the origin of `manage@link-url`), all settings, not secrets
//!   ([`identity_env`]). Its ingest key is made in Monitor's format ([`new_ingest_key`]), deposited as the vault item
//!   `AppInstance:{id}:ingestKey` and given to the container as a file its export reads (`telemetry@otlp-headers-file`),
//!   never in its environment; once every instance of the provision is up the key is registered here as an `IngestKey`
//!   of source `instance:{id}`, owned by the user who provisioned it, who must hold `MONITOR_INGEST` ([`register`]). A
//!   deploy sets `telemetry@service-version` ([`version_of`]).
//! - **Scrape targets.** A kind with a `scrapeUrlTemplate` (Postgres, MySQL and OpenSearch, through their exporters)
//!   gets a `ScrapeTarget` per instance, whose metrics carry the instance's id ([`register`]). Removing the instance
//!   revokes its key and removes its target ([`unregister`]).
//! - **What the shipped alert rules read** (`tools/report/instance-*.json`): gauges of each instance, from a snapshot of
//!   the rows that each member reads again at most every 30 seconds and after a link change ([`refresh`],
//!   [`mark_changed`]), computed at collection time: how long
//!   it has been sealed, how long its link has been down, whether its check fails, and the items past their rotation
//!   age (M5 fills them); and counters of what happened: each unseal (and whether from an address the instance never
//!   unsealed from before, [`note_seal`]), each failed deploy, and each use of machine access outside a deploy.
//! - **Clusters.** The live members of a managed instance's cluster, as its link reports them, are kept on its row
//!   ([`note_members`]): with the identity above they are the `AppInstance` ids Monitor's per-instance comparison sees.

use std::sync::Mutex;

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{Value, ValueMap};
use moqui_lib::sync::lock;
use sha2::Digest as _;
use zeroize::Zeroizing;

use crate::common::{fail, field, find, one, rows, APP_INSTANCE, ENVIRONMENT};
use crate::plan::{Plan, Step};

/// The kind secret that is an instance's Monitor ingest key.
pub const INGEST_SECRET: &str = "ingestKey";
/// The actions that are deploys: a failed one is counted, and machine access used by one is expected.
pub const DEPLOY_ACTIONS: &[&str] = &["IatDeploy", "IatUpgrade", "IatRollback"];
/// The same, by the verb an action row is recorded with.
const DEPLOY_VERBS: &[&str] = &["deploy", "upgrade", "rollback"];

const INGEST_KEY: &str = "moqui.monitor.IngestKey";
const SCRAPE_TARGET: &str = "moqui.monitor.ScrapeTarget";
/// How long an instance's ingest key lasts; rotation (M5) makes a new one before it ends.
const INGEST_KEY_DAYS: i64 = 365;
/// The most cluster members of an instance's report that its row keeps.
const MAX_MEMBERS: usize = 256;
/// The most addresses an instance's row keeps of where it unsealed from.
const MAX_ADDRESSES: usize = 20;

/// A new ingest key in Monitor's format (`mqi_` and 32 random bytes, base64url): what Monitor's receiver takes.
#[must_use]
pub fn new_ingest_key() -> Zeroizing<String> {
    use base64::Engine as _;
    let mut secret = Zeroizing::new([0_u8; 32]);
    moqui_vault::rng::Rng::fill(&mut moqui_vault::rng::OsRng, &mut secret[..]);
    Zeroizing::new(format!(
        "mqi_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&secret[..])
    ))
}

/// The `IngestKey` row id of a key: its SHA-256, as Monitor keeps it.
#[must_use]
pub fn ingest_key_id(key: &str) -> String {
    moqui_vault::text::hex(&sha2::Sha256::digest(key.as_bytes()))
}

/// The ingest source of an instance, which Monitor › Sources shows and which names it on each record it sends.
#[must_use]
pub fn ingest_source(instance_id: &str) -> String {
    format!("instance:{instance_id}")
}

/// The id of an instance's scrape target.
#[must_use]
pub fn scrape_target_id(instance_id: &str) -> String {
    format!("{instance_id}-scrape")
}

/// The origin (`scheme://host[:port]`) of a URL and its host, lower case; None when it is none.
fn origin(url: &str) -> Option<(String, String)> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let host = match authority.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => authority.rsplit_once(':').map_or(authority, |(host, _)| host),
    };
    Some((
        format!("{}://{authority}", scheme.to_ascii_lowercase()),
        host.to_ascii_lowercase(),
    ))
}

/// Whether `host` is this machine's loopback, where plain http is allowed.
fn loopback(host: &str) -> bool {
    host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Where this manager's instances send their telemetry, with their ingest key: `manage@monitor-url`, else the origin of
/// `manage@link-url`. It must be https (http only to the loopback), and on the link's host unless
/// `manage@monitor-other-host` is true in the conf: an instance's ingest key goes nowhere else. None (logged) when it is
/// not, and when neither is set.
#[must_use]
pub fn monitor_url() -> Option<String> {
    let link = crate::link::manager().and_then(|(url, _)| origin(&url));
    let set = moqui_core::context::conf_env::setting("manage@monitor-url");
    let (url, host) = if set.trim().is_empty() {
        link.clone()?
    } else {
        let Some(found) = origin(&set) else {
            tracing::error!("Instances: manage@monitor-url {set:?} is not a URL: provisions give no OTLP address");
            return None;
        };
        found
    };
    if !url.starts_with("https://") && !(url.starts_with("http://") && loopback(&host)) {
        tracing::error!(
            "Instances: the Monitor address {url} is not https (http only to the loopback): provisions give no OTLP \
             address"
        );
        return None;
    }
    let other_host_allowed = moqui_core::context::conf_env::setting("manage@monitor-other-host") == "true";
    if let Some((_, link_host)) = &link {
        if *link_host != host && !other_host_allowed {
            tracing::error!(
                "Instances: manage@monitor-url's host {host} is not manage@link-url's {link_host} (set \
                 manage@monitor-other-host to allow it): provisions give no OTLP address"
            );
            return None;
        }
    }
    Some(url)
}

/// The version an instance reports as `service.version`: its image digest's short form and, with component commits,
/// a short hash of them; none before its first deploy records one.
#[must_use]
pub fn version_of(instance: &EntityValue) -> String {
    let digest = field(instance, "imageDigest");
    let image = digest
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .chars()
        .take(12)
        .collect::<String>();
    let commits = field(instance, "componentCommits");
    if commits.is_empty() {
        return image;
    }
    let hash = moqui_vault::text::hex(&sha2::Sha256::digest(commits.as_bytes()));
    format!("{image}+{}", &hash[..8])
}

/// The settings that give an mr-moqui instance its Monitor identity (see the module doc); none for another kind.
#[must_use]
pub fn identity_env(facade: &EntityFacade, plan: &Plan, step: &Step) -> Vec<(String, String)> {
    if field(&step.kind, "instanceKindId") != "mr-moqui" {
        return Vec::new();
    }
    let _ = facade;
    let mut env = vec![
        ("moqui_conf__instance__id".to_string(), step.id()),
        (
            "moqui_conf__telemetry__deployment_environment".to_string(),
            plan.environment_id.clone(),
        ),
        // an environment's instances are one service, apart from this manager's own (whose name Monitor's receiver
        // would turn a sender's into `<name> (received)`)
        (
            "moqui_conf__telemetry__service_name".to_string(),
            format!("moqui-{}", plan.environment_id),
        ),
    ];
    let version = version_of(&step.instance);
    if !version.is_empty() {
        env.push(("moqui_conf__telemetry__service_version".to_string(), version));
    }
    if let Some(url) = monitor_url() {
        env.push(("moqui_conf__telemetry__otlp_endpoint".to_string(), url));
    }
    env
}

/// Registers what a provision made with Monitor, once every instance is up (§4.3 step 6): each instance's ingest key
/// and each scrape target of its kind. A registration that fails is logged and named in the report, and the rest go on:
/// the instances are made.
pub fn register(
    facade: &EntityFacade,
    plan: &Plan,
    ingest_keys: &[(String, Zeroizing<String>)],
    report: &mut Vec<String>,
) {
    for (instance_id, key) in ingest_keys {
        match register_ingest_key(facade, instance_id, key) {
            Ok(line) => report.push(line),
            Err(err) => {
                tracing::error!("Instances: the ingest key of AppInstance {instance_id} was not registered: {err}");
                report.push(format!(
                    "AppInstance {instance_id}: its ingest key was not registered with Monitor: {}",
                    err.message()
                ));
            }
        }
    }
    for step in &plan.steps {
        match register_scrape_target(facade, step) {
            Ok(Some(line)) => report.push(line),
            Ok(None) => {}
            Err(err) => {
                tracing::error!(
                    "Instances: the scrape target of AppInstance {} was not added: {err}",
                    step.id()
                );
                report.push(format!(
                    "AppInstance {}: its scrape target was not added to Monitor: {}",
                    step.id(),
                    err.message()
                ));
            }
        }
    }
}

/// The `IngestKey` row of an instance's key, owned by the signed-in user, who must hold `MONITOR_INGEST` (Monitor's rule
/// for a key's owner); an earlier key of the instance is revoked.
fn register_ingest_key(facade: &EntityFacade, instance_id: &str, key: &str) -> Result<String, LibError> {
    let user = moqui_core::user::current_user_id()
        .ok()
        .flatten()
        .ok_or_else(|| fail("no user is signed in to own the key: register it in Monitor › Sources"))?;
    if !moqui_core::user::has_permission("MONITOR_INGEST").unwrap_or(false) {
        return Err(fail(format!(
            "{user} does not hold MONITOR_INGEST, which an ingest key's owner needs: provision as a user who does"
        )));
    }
    let key_id = ingest_key_id(key);
    let source = ingest_source(instance_id);
    for mut earlier in find(facade, INGEST_KEY)?
        .eq("source", Value::string(&source))
        .list()?
        .into_values()
    {
        if field(&earlier, "revoked") != "Y" {
            earlier.set("revoked", Value::string("Y"));
            earlier.update(facade)?;
        }
    }
    let now = moqui_core::user::now_millis();
    let mut row = facade.make_value(INGEST_KEY)?;
    row.set("keyId", Value::string(&key_id));
    row.set("source", Value::string(&source));
    row.set("userId", Value::string(&user));
    row.set("createdDate", moqui_core::user::stamp_value(now));
    row.set(
        "expiresDate",
        moqui_core::user::stamp_value(now + INGEST_KEY_DAYS * 86_400_000),
    );
    row.set("revoked", Value::string("N"));
    row.create(facade)?;
    Ok(format!(
        "registered the ingest key of AppInstance {instance_id} with Monitor (source {source})"
    ))
}

/// The `ScrapeTarget` of an instance whose kind names a `scrapeUrlTemplate`, made or updated; None for a kind with
/// none. It reaches a private address (the environment's network) only when the user holds
/// `MONITOR_PRIVATE_NETWORK`, as Monitor › Checks asks; otherwise only on an instance that is not production.
fn register_scrape_target(facade: &EntityFacade, step: &Step) -> Result<Option<String>, LibError> {
    let template = field(&step.kind, "scrapeUrlTemplate");
    if template.is_empty() {
        return Ok(None);
    }
    let instance_id = step.id();
    let name = field(&step.instance, "instanceName");
    let values = [
        ("appInstanceId", instance_id.clone()),
        ("instanceName", name.clone()),
        ("hostName", field(&step.instance, "hostName")),
    ]
    .into_iter()
    .collect();
    let url = crate::kinds::expand(&template, &values)?;
    let target_id = scrape_target_id(&instance_id);
    let existing = one(facade, SCRAPE_TARGET, "targetId", &target_id)?;
    // a target of that id a person made (it names no instance, or another) is theirs: not taken over
    if let Some(theirs) = existing
        .as_ref()
        .filter(|row| field(row, "serviceInstanceId") != instance_id)
    {
        return Err(fail(format!(
            "the scrape target {target_id} is there already and is not this instance's ({:?}): it is left as it is",
            field(theirs, "url")
        )));
    }
    let mut row = match &existing {
        Some(row) => row.clone(),
        None => {
            let mut row = facade.make_value(SCRAPE_TARGET)?;
            row.set("targetId", Value::string(&target_id));
            row
        }
    };
    row.set("url", Value::string(&url));
    row.set(
        "serviceName",
        Value::string(if name.is_empty() { &instance_id } else { &name }),
    );
    row.set("serviceInstanceId", Value::string(&instance_id));
    row.set("intervalSeconds", Value::I64(60));
    row.set("timeoutSeconds", Value::I64(10));
    row.set("enabled", Value::string("Y"));
    let private = moqui_core::user::has_permission("MONITOR_PRIVATE_NETWORK").unwrap_or(false);
    row.set("allowPrivateNetwork", Value::string(if private { "Y" } else { "N" }));
    if existing.is_some() {
        row.update(facade)?;
    } else {
        row.create(facade)?;
    }
    Ok(Some(format!(
        "added the scrape target {target_id} ({url}) of AppInstance {instance_id} to Monitor"
    )))
}

/// Takes an instance out of Monitor: its ingest keys revoked and its scrape target removed. The report's lines; a
/// failure is logged and named there.
pub fn unregister(facade: &EntityFacade, instance_id: &str) -> Vec<String> {
    let mut report = Vec::new();
    let done = (|| -> Result<(usize, bool), LibError> {
        let mut revoked = 0;
        for mut key in find(facade, INGEST_KEY)?
            .eq("source", Value::string(ingest_source(instance_id)))
            .list()?
            .into_values()
        {
            if field(&key, "revoked") != "Y" {
                key.set("revoked", Value::string("Y"));
                key.update(facade)?;
                revoked += 1;
            }
        }
        // only the target the provision made, which names the instance; one a person made of that id stays
        let target = one(facade, SCRAPE_TARGET, "targetId", &scrape_target_id(instance_id))?
            .filter(|row| field(row, "serviceInstanceId") == instance_id);
        let removed = target.is_some();
        if let Some(target) = target {
            target.delete(facade)?;
        }
        Ok((revoked, removed))
    })();
    match done {
        Ok((revoked, removed)) => {
            if revoked > 0 {
                report.push(format!("revoked {revoked} Monitor ingest key(s)"));
            }
            if removed {
                report.push("removed the Monitor scrape target".to_string());
            }
        }
        Err(err) => {
            tracing::error!("Instances: AppInstance {instance_id} was not taken out of Monitor: {err}");
            report.push(format!("not taken out of Monitor: {}", err.message()));
        }
    }
    report
}

// ----- what the alert rules read -----

/// One instance as the gauges see it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reading {
    pub instance_id: String,
    pub environment_id: String,
    /// Since when its link has said it is sealed (epoch milliseconds).
    pub sealed_since: Option<i64>,
    /// Since when its link has been down, for an instance that paired.
    pub link_down_since: Option<i64>,
    /// It should run, and its host says it does not, or its check fails.
    pub check_failing: bool,
}

impl Reading {
    /// Seconds sealed at `now`; 0 while unsealed.
    #[must_use]
    pub fn sealed_seconds(&self, now: i64) -> f64 {
        self.sealed_since
            .map_or(0.0, |since| (now - since).max(0) as f64 / 1000.0)
    }

    /// Seconds its link has been down at `now`; 0 while linked.
    #[must_use]
    pub fn link_down_seconds(&self, now: i64) -> f64 {
        self.link_down_since
            .map_or(0.0, |since| (now - since).max(0) as f64 / 1000.0)
    }
}

/// How long a snapshot of the rows serves the gauges before they read the rows again, on each member of a manager
/// cluster: a member that did not see a link change itself (another member holds that link) is behind by at most this.
const SNAPSHOT_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(30);
/// How long after its pairing an instance that never linked counts as down.
pub const NEVER_LINKED_GRACE_MILLIS: i64 = 10 * 60_000;

/// The last snapshot of the instances, which the gauges read: when it was read, and whether a change was seen since.
#[derive(Default)]
struct Snapshot {
    read_at: Option<std::time::Instant>,
    dirty: bool,
    readings: Vec<Reading>,
}

static SNAPSHOT: Mutex<Snapshot> = Mutex::new(Snapshot {
    read_at: None,
    dirty: false,
    readings: Vec::new(),
});

/// The facade the gauges read the rows with, set at the start of the server.
static FACADE: Mutex<Option<EntityFacade>> = Mutex::new(None);

fn millis_of(row: &EntityValue, name: &str) -> Option<i64> {
    row.get(name).and_then(moqui_core::user::value_millis)
}

/// What the gauges read of each instance that has a container and is not removed. An instance that paired is down
/// while its link says down or unknown (since then), or never linked past [`NEVER_LINKED_GRACE_MILLIS`].
pub fn readings(facade: &EntityFacade) -> Result<Vec<Reading>, LibError> {
    let now = moqui_core::user::now_millis();
    let mut out = Vec::new();
    for row in find(facade, APP_INSTANCE)?.list()?.into_values() {
        if field(&row, "instanceUuid").is_empty() || field(&row, "desiredStateEnumId") == "IdsRemoved" {
            continue;
        }
        let seal = field(&row, "sealState");
        let sealed_since = (!seal.is_empty() && seal != "unsealed")
            .then(|| millis_of(&row, "sealedSince").or_else(|| millis_of(&row, "sealCheckedDate")))
            .flatten();
        let paired = millis_of(&row, "linkPairedDate");
        let link_down_since = match (paired, field(&row, "linkState").as_str()) {
            (None, _) => None,
            (Some(_), "down" | "unknown") => millis_of(&row, "linkCheckedDate"),
            (Some(at), "") => Some(at + NEVER_LINKED_GRACE_MILLIS).filter(|since| *since <= now),
            _ => None,
        };
        let running = field(&row, "desiredStateEnumId") == "IdsRunning";
        let observed = field(&row, "observedState");
        let check_failing =
            running && !observed.is_empty() && (observed != "running" || field(&row, "observedHealth") == "unhealthy");
        out.push(Reading {
            instance_id: field(&row, "appInstanceId"),
            environment_id: field(&row, "environmentId"),
            sealed_since,
            link_down_since,
            check_failing,
        });
    }
    Ok(out)
}

/// Reads the instances again for the gauges; a failure is logged and the last snapshot stays.
pub fn refresh(facade: &EntityFacade) {
    match readings(facade) {
        Ok(readings) => {
            let mut snapshot = lock(&SNAPSHOT);
            snapshot.readings = readings;
            snapshot.read_at = Some(std::time::Instant::now());
            snapshot.dirty = false;
        }
        Err(err) => tracing::error!("Instances: the Monitor snapshot of the instances was not read: {err}"),
    }
}

/// A link or seal changed: the gauges read the rows again at their next collection, once however many changed (a storm
/// of reconnects reads them once).
pub fn mark_changed() {
    lock(&SNAPSHOT).dirty = true;
}

/// The snapshot's readings, read again from the rows first when a change was seen or it is older than
/// [`SNAPSHOT_MAX_AGE`] and the server's facade is known.
fn current() -> Vec<Reading> {
    let stale = {
        let snapshot = lock(&SNAPSHOT);
        snapshot.dirty || snapshot.read_at.is_none_or(|at| at.elapsed() > SNAPSHOT_MAX_AGE)
    };
    if stale {
        let facade = lock(&FACADE).clone();
        if let Some(facade) = facade {
            refresh(&facade);
        }
    }
    lock(&SNAPSHOT).readings.clone()
}

/// The snapshot's series of one gauge at `now`.
#[must_use]
pub fn series(gauge: &str, now: i64) -> Vec<(ValueMap, f64)> {
    let snapshot = current();
    match gauge {
        "moqui.instance.items_past_rotation" => {
            // the rotation ages are M5's; until then each environment reports none past its age
            let mut environments: Vec<String> = snapshot.iter().map(|reading| reading.environment_id.clone()).collect();
            environments.sort_unstable();
            environments.dedup();
            environments
                .into_iter()
                .map(|environment| (attributes(None, &environment, &[]), 0.0))
                .collect()
        }
        _ => snapshot
            .iter()
            .map(|reading| {
                let value = match gauge {
                    "moqui.instance.sealed_seconds" => reading.sealed_seconds(now),
                    "moqui.instance.link_down_seconds" => reading.link_down_seconds(now),
                    _ => f64::from(u8::from(reading.check_failing)),
                };
                (
                    attributes(Some(&reading.instance_id), &reading.environment_id, &[]),
                    value,
                )
            })
            .collect(),
    }
}

/// The gauges the shipped instance alert rules read.
pub const GAUGES: &[&str] = &[
    "moqui.instance.sealed_seconds",
    "moqui.instance.link_down_seconds",
    "moqui.instance.check_failing",
    "moqui.instance.items_past_rotation",
];

/// Registers the gauges with this server's telemetry, which read the rows through its facade (the extension's start).
pub fn start(factory: &moqui_core::context::ExecutionContextFactory) {
    let telemetry = moqui_core::telemetry::for_factory(factory);
    for gauge in GAUGES {
        telemetry.observe_gauge_series(gauge, move || series(gauge, moqui_core::user::now_millis()));
    }
    if let Some(facade) = factory.entity() {
        *lock(&FACADE) = Some(facade.clone());
        refresh(&facade);
    }
}

/// Once this member's cluster is up: a manager that is alone in its cluster holds no link after its restart, so each
/// instance its rows say is linked is down until it links again, and the link-down rule sees one that does not. In a
/// cluster with other live members the links they hold stay as they are.
pub fn after_cluster_start(factory: &moqui_core::context::ExecutionContextFactory) {
    let Some(facade) = factory.entity() else {
        return;
    };
    let own = moqui_core::cluster::instance_id(factory).unwrap_or_default();
    let others = moqui_core::cluster::member_report(factory, &facade)
        .map(|members| members.iter().any(|member| member.live && member.instance_id != own))
        .unwrap_or(true);
    if others {
        return;
    }
    let marked = (|| -> Result<usize, LibError> {
        let mut marked = 0;
        for mut row in find(&facade, APP_INSTANCE)?
            .eq("linkState", Value::string("linked"))
            .list()?
            .into_values()
        {
            row.set("linkState", Value::string("unknown"));
            row.set("linkCheckedDate", moqui_core::user::now_value());
            row.update(&facade)?;
            marked += 1;
        }
        Ok(marked)
    })();
    match marked {
        Ok(0) => {}
        Ok(count) => tracing::info!("Instances: {count} links are unknown until their instances link again"),
        Err(err) => tracing::error!("Instances: the links held before the restart were not marked: {err}"),
    }
    mark_changed();
}

fn attributes(instance_id: Option<&str>, environment_id: &str, more: &[(&str, &str)]) -> ValueMap {
    let mut map = ValueMap::default();
    if let Some(instance_id) = instance_id {
        map.insert("moqui.instance.id".to_string(), Value::string(instance_id));
    }
    map.insert("moqui.instance.environment".to_string(), Value::string(environment_id));
    for (name, value) in more {
        map.insert((*name).to_string(), Value::string(*value));
    }
    map
}

/// Adds one to an instance counter of this server's telemetry.
fn count(name: &str, instance_id: &str, environment_id: &str, more: &[(&str, &str)]) {
    if let Some(telemetry) = moqui_core::telemetry::current() {
        telemetry.add_counter(name, 1, attributes(Some(instance_id), environment_id, more));
    }
}

/// An unseal its link reported (§4.8: "an unseal"; "an unseal from an address the instance has not used before").
pub fn count_unseal(instance_id: &str, environment_id: &str, new_address: bool) {
    count(
        "moqui.instance.unseals",
        instance_id,
        environment_id,
        &[("moqui.instance.new_address", if new_address { "true" } else { "false" })],
    );
    if new_address {
        tracing::warn!("Instances: AppInstance {instance_id} unsealed from an address it never unsealed from before");
    }
}

/// An action ended: a failed deploy is counted (§4.8: "a failed deploy").
pub fn note_action_end(facade: &EntityFacade, action: &EntityValue, status: &str) {
    let action_enum = field(action, "actionEnumId");
    if status == crate::actions::FAILED && DEPLOY_ACTIONS.contains(&action_enum.as_str()) {
        let instance_id = field(action, "appInstanceId");
        // a waiting action and the action it ran under its lease are one deploy: counted once, by the first to fail
        let lease_id = field(action, "leaseId");
        if !lease_id.is_empty() {
            let counted = find(facade, crate::common::ACTION)
                .and_then(|find| find.eq("leaseId", Value::string(&lease_id)).list())
                .map(|rows| {
                    rows.into_values().iter().any(|other| {
                        field(other, "instanceActionId") != field(action, "instanceActionId")
                            && field(other, "statusEnumId") == crate::actions::FAILED
                            && field(other, "actionEnumId") == action_enum
                    })
                })
                .unwrap_or(false);
            if counted {
                return;
            }
        }
        count(
            "moqui.instance.deploys_failed",
            &instance_id,
            &environment_id(facade, &instance_id),
            &[("moqui.instance.action", &action_enum)],
        );
    }
}

/// An action ran under the environment's machine access: counted when it is not a deploy (§4.8: "machine access used
/// outside a deploy").
pub fn note_machine_access(facade: &EntityFacade, instance_id: &str, verb: &str) {
    if !DEPLOY_VERBS.contains(&verb) {
        count(
            "moqui.instance.machine_access",
            instance_id,
            &environment_id(facade, instance_id),
            &[("moqui.instance.action", verb)],
        );
        tracing::warn!("Instances: machine access was used to {verb} AppInstance {instance_id}, outside a deploy");
    }
}

/// What a change of seal state does to the instance's row (before it is written): since when it is sealed, and, at an
/// unseal, the address it came from kept among those it used. Some(new address) at an unseal: whether the address is
/// one it never unsealed from before (its first unseal sets the first address, and is not new).
pub fn note_seal(row: &mut EntityValue, seal_state: &str, address: &str) -> Option<bool> {
    let was = field(row, "sealState");
    if seal_state != "unsealed" {
        if was == "unsealed" || was.is_empty() || field(row, "sealedSince").is_empty() {
            row.set("sealedSince", moqui_core::user::now_value());
        }
        return None;
    }
    row.set("sealedSince", Value::Null);
    if was == "unsealed" {
        return None;
    }
    let address = Some(address.to_string())
        .filter(|address| !address.is_empty())
        .unwrap_or_else(|| field(row, "linkAddress"));
    let mut known: Vec<String> = field(row, "unsealAddresses")
        .split(',')
        .map(str::trim)
        .filter(|known| !known.is_empty())
        .map(str::to_string)
        .collect();
    if address.is_empty() {
        return Some(false);
    }
    let new = !known.is_empty() && !known.contains(&address);
    known.retain(|known| *known != address);
    known.push(address);
    let skip = known.len().saturating_sub(MAX_ADDRESSES);
    row.set("unsealAddresses", Value::string(known[skip..].join(",")));
    Some(new)
}

/// The live members of an instance's cluster as its link reported them, kept on its row when they changed; a failure
/// is logged.
pub fn note_members(facade: &EntityFacade, instance_id: &str, members: &[String]) {
    let _authz = crate::common::authz_off();
    let written = (|| -> Result<(), LibError> {
        let mut row = crate::plan::instance(facade, instance_id)?;
        // an instance's own report: at most MAX_MEMBERS ids of at most 128 characters
        let mut sorted: Vec<String> = members
            .iter()
            .take(MAX_MEMBERS)
            .map(|member| member.chars().take(128).collect())
            .collect();
        sorted.sort_unstable();
        let text = sorted.join(",");
        if field(&row, "clusterMembers") != text {
            row.set("clusterMembers", Value::string(text));
            row.update(facade)?;
        }
        Ok(())
    })();
    if let Err(err) = written {
        tracing::error!("Instances: the cluster members of AppInstance {instance_id} were not written: {err}");
    }
}

/// The environment of an instance by its row, for a counter's attribute; empty when it has none.
pub(crate) fn environment_id(facade: &EntityFacade, instance_id: &str) -> String {
    one(facade, APP_INSTANCE, "appInstanceId", instance_id)
        .ok()
        .flatten()
        .map(|row| field(&row, "environmentId"))
        .unwrap_or_default()
}

/// Whether an environment's rolling deploys are paused (Monitor's pause-rollout): a deploy changes no further member
/// while they are.
pub fn rollout_paused(facade: &EntityFacade, environment_id: &str) -> Result<bool, LibError> {
    Ok(one(facade, ENVIRONMENT, "environmentId", environment_id)?
        .is_some_and(|row| !field(&row, "rolloutPausedDate").is_empty()))
}

/// The ingest keys of an instance, newest last.
pub(crate) fn ingest_keys(facade: &EntityFacade, instance_id: &str) -> Result<Vec<EntityValue>, LibError> {
    rows(facade, INGEST_KEY, "source", &ingest_source(instance_id))
}
