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

//! What a job uploads (docs/interfaces/Pipelines.md "Artifacts, reports and caches"): artifacts, JUnit reports and cache
//! archives, each a content blob streamed in with a size limit, and a `GitRunArtifact` row with its size, checksum
//! and expiry. A report's failed tests become `GitRunTestFailure` rows. A cache is per repository and key, kept apart
//! for protected refs: a run for a protected ref reads only caches such a run wrote, an untrusted run reads only
//! caches no protected run wrote and writes none. What a repository and an owner keep is capped (`git-ci`
//! `max-repository-upload-bytes`, `max-owner-upload-bytes`, and for an untrusted run `max-untrusted-run-upload-bytes`),
//! and every upload a job makes counts toward `max-artifacts`. The daily retention removes what expired;
//! `gc#ContentBlobs` then deletes the blobs.

use std::io::Read;

use moqui_lib::object::Value;

use moqui_core::context::Error;
use moqui_core::entity::{Condition, EntityFacade, EntityValue};
use moqui_core::resource::blob::{BlobInfo, BlobStore, Expected};

use super::{field, int, is_yes, long, millis, opt, yes_no, JOB_RUN, LOG_CHUNK, RUN_ARTIFACT, TEST_FAILURE};

const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;
/// `git-ci` `max-repository-upload-bytes` when the conf does not say: 10 GiB.
const DEFAULT_REPOSITORY_UPLOAD_BYTES: u64 = 10 * 1024 * 1024 * 1024;
/// `git-ci` `max-untrusted-run-upload-bytes` when the conf does not say: 1 GiB.
const DEFAULT_UNTRUSTED_RUN_UPLOAD_BYTES: u64 = 1024 * 1024 * 1024;

/// A storage cap of the conf: its number, 0 for no cap, or `default` when it is not a number.
fn cap_setting(name: &str, default: u64) -> u64 {
    super::setting(name).trim().parse::<u64>().unwrap_or(default)
}

/// The failed tests kept of one job.
const MAX_FAILURES: usize = 500;

/// What is uploaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Artifact,
    Report,
    Cache,
}

impl Kind {
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "artifact" => Self::Artifact,
            "report" => Self::Report,
            "cache" => Self::Cache,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Artifact => "artifact",
            Self::Report => "report",
            Self::Cache => "cache",
        }
    }

    /// The most bytes of one upload: the conf's `git-ci` `max-artifact-bytes` (1 GB), `max-report-bytes` (16 MB)
    /// or `max-cache-bytes` (2 GB).
    pub fn max_bytes(self) -> u64 {
        match self {
            Self::Artifact => super::number_setting("max-artifact-bytes", 1024 * 1024 * 1024),
            Self::Report => super::number_setting("max-report-bytes", 16 * 1024 * 1024),
            Self::Cache => super::number_setting("max-cache-bytes", 2 * 1024 * 1024 * 1024),
        }
    }
}

/// An upload of a job.
#[derive(Clone, Debug)]
pub struct Upload<'a> {
    pub kind: Kind,
    pub name: &'a str,
    pub path: Option<&'a str>,
    /// A cache's key, as the runner expanded it.
    pub cache_key: Option<&'a str>,
    pub retention_days: Option<i64>,
    /// The size the upload says it is (its `Content-Length`), when it says: the room a cap makes for it by evicting
    /// caches.
    pub size: Option<u64>,
}

fn days(name: &str, default: u64) -> i64 {
    i64::try_from(super::number_setting(name, default)).unwrap_or(i64::MAX / DAY_MILLIS)
}

/// The cache a run reads for `key`: one a run of its own class wrote, and for a trusted run that is not for a
/// protected ref, one a protected run wrote when there is no other. An untrusted run never reads a protected run's
/// cache, which a build with secrets may have filled.
pub fn find_cache(entity: &EntityFacade, run: &EntityValue, key: &str) -> Result<Option<EntityValue>, Error> {
    let _rows = crate::rows_guard();
    let protected = is_yes(run, "protectedRef");
    let trusted = is_yes(run, "trusted");
    let rows = entity
        .find(RUN_ARTIFACT)?
        .eq("repositoryId", opt(field(run, "repositoryId")))
        .eq("artifactKind", Value::string("cache"))
        .eq("cacheKey", Value::string(key))
        .order_by("createdDate", true)
        .list()?;
    let mut rows: Vec<&EntityValue> = rows
        .iter()
        .filter(|row| !protected || is_yes(row, "protectedRef"))
        .filter(|row| trusted || !is_yes(row, "protectedRef"))
        .collect();
    rows.sort_by_key(|row| is_yes(row, "protectedRef") != protected);
    Ok(rows.first().map(|row| (*row).clone()))
}

fn record_failures(entity: &EntityFacade, job_run_id: &str, blobs: &BlobStore, info: &BlobInfo) -> Result<(), Error> {
    let Some(bytes) = blobs.read(&info.id)? else {
        return Ok(());
    };
    let report = match super::junit::parse(&String::from_utf8_lossy(&bytes)) {
        Ok(report) => report,
        Err(err) => {
            tracing::warn!(job = %job_run_id, "a JUnit report was not read: {err}");
            return Ok(());
        }
    };
    let Some(job) = super::one(entity, JOB_RUN, "jobRunId", job_run_id)? else {
        return Ok(());
    };
    let known = long(&job, "testFailedCount").unwrap_or(0);
    let mut seq = known;
    for failure in &report.failures {
        if usize::try_from(seq).unwrap_or(usize::MAX) >= MAX_FAILURES {
            break;
        }
        seq += 1;
        let mut row = EntityValue::make(entity, TEST_FAILURE)?;
        row.set("jobRunId", Value::string(job_run_id));
        row.set("testSeq", int(seq));
        row.set("suiteName", Value::string(&failure.suite));
        row.set("testName", Value::string(&failure.test));
        row.set("filePath", opt(failure.file.clone()));
        row.set("lineNumber", failure.line.map_or(Value::Null, int));
        row.set("message", Value::string(&failure.message));
        row.set("durationMillis", failure.duration_millis.map_or(Value::Null, int));
        row.create(entity)?;
    }
    super::set_fields(
        entity,
        JOB_RUN,
        "jobRunId",
        job_run_id,
        None,
        vec![
            (
                "testCount",
                int(long(&job, "testCount").unwrap_or(0) + i64::try_from(report.tests).unwrap_or(0)),
            ),
            (
                "testFailedCount",
                int(known + i64::try_from(report.failures.len()).unwrap_or(0)),
            ),
        ],
    )?;
    Ok(())
}

/// Store an upload of the running job `job` of `run` from `input`, at most the kind's size limit. The artifact's id;
/// `None` for a cache whose key is already stored (the first one written stands).
///
/// # Errors
///
/// For a cache of an untrusted run, a job with too many uploads, a body over the limit.
pub fn store(
    entity: &EntityFacade,
    run: &EntityValue,
    job: &EntityValue,
    upload: &Upload<'_>,
    input: &mut dyn Read,
) -> Result<Option<String>, Error> {
    let _rows = crate::rows_guard();
    let job_run_id = field(job, "jobRunId").unwrap_or_default();
    // a cache of a job that was given secrets (an environment's allowedRefs on a branch no rule protects) is of the
    // protected class too: a build with secrets may have left them in it, and no untrusted run reads it (find_cache)
    let protected = is_yes(run, "protectedRef")
        || (upload.kind == Kind::Cache && {
            let spec: serde_json::Value =
                serde_json::from_str(&field(job, "jobSpec").unwrap_or_default()).unwrap_or(serde_json::Value::Null);
            !super::trust::secrets_for(entity, run, &spec, false)?.is_empty()
        });
    if upload.kind == Kind::Cache {
        if !is_yes(run, "trusted") {
            return Err(Error::new("an untrusted run reads caches and writes none"));
        }
        let Some(key) = upload.cache_key.filter(|key| !key.is_empty() && key.len() <= 200) else {
            return Err(Error::new("a cache needs a key of at most 200 characters"));
        };
        let stored = entity
            .find(RUN_ARTIFACT)?
            .eq("repositoryId", opt(field(run, "repositoryId")))
            .eq("artifactKind", Value::string("cache"))
            .eq("cacheKey", Value::string(key))
            .eq("protectedRef", yes_no(protected))
            .limit(1)
            .list()?;
        if !stored.is_empty() {
            return Ok(None);
        }
    }
    // every upload of a job counts, its caches too
    let max = i64::try_from(super::number_setting("max-artifacts", 100)).unwrap_or(100);
    let count = entity
        .find(RUN_ARTIFACT)?
        .eq("jobRunId", Value::string(&job_run_id))
        .count()?;
    if count >= max {
        return Err(Error::new(format!(
            "a job uploads at most {max} artifacts, reports and caches"
        )));
    }
    // what the repository and the run's owner keep, each against its cap; the upload is cut at what is left
    let owner = field(run, "ownerId");
    let mut room = upload.kind.max_bytes();
    // the cap that cuts the upload, when one does: an upload past it is refused as the quota's, not the kind's limit
    let mut capped: Option<(&str, &str, u64)> = None;
    // an untrusted run (a fork's, a review push's, an approved one of an account without update) keeps a smaller total
    let untrusted = (!is_yes(run, "trusted")).then(|| field(run, "pipelineRunId")).flatten();
    let caps = [
        (
            "max-repository-upload-bytes",
            "repositoryId",
            field(run, "repositoryId"),
            "repository",
            DEFAULT_REPOSITORY_UPLOAD_BYTES,
        ),
        ("max-owner-upload-bytes", "ownerId", owner.clone(), "owner", 0),
        (
            "max-untrusted-run-upload-bytes",
            "pipelineRunId",
            untrusted,
            "untrusted run",
            DEFAULT_UNTRUSTED_RUN_UPLOAD_BYTES,
        ),
    ];
    for (setting, key, id, what, default) in &caps {
        let cap = cap_setting(setting, *default);
        let Some(id) = id.as_ref().filter(|_| cap > 0) else {
            continue;
        };
        if *key == "ownerId" {
            backfill_owners(entity);
        }
        let mut used = u64::try_from(stored_bytes(entity, key, id)?).unwrap_or(0);
        // caches count toward the cap and are kept while they are used, so room for the upload is made by evicting
        // the least recently used ones (SEC-GIT-90); an artifact or a report is never evicted
        let wanted = upload.size.unwrap_or_else(|| upload.kind.max_bytes()).min(cap);
        if used.saturating_add(wanted) > cap {
            used = evict_caches(entity, key, id, used, cap.saturating_sub(wanted))?;
        }
        if used >= cap {
            return Err(quota(what, used, cap, setting));
        }
        if cap - used < room {
            room = cap - used;
            capped = Some((what, setting, cap));
        }
    }
    let blobs = BlobStore::new(entity.clone());
    let info = match blobs.put(input, &Expected::default(), room) {
        Ok(info) => info,
        Err(err) => {
            return Err(match capped {
                Some((what, setting, cap)) => quota(what, cap.saturating_sub(room), cap, setting),
                None => err.into(),
            })
        }
    };
    let now = super::now_millis();
    let retention = match upload.kind {
        Kind::Artifact => upload
            .retention_days
            .filter(|days| *days > 0)
            .unwrap_or_else(|| days("artifact-retention-days", 30))
            .min(days("max-artifact-retention-days", 365)),
        Kind::Report => days("log-retention-days", 90),
        Kind::Cache => days("cache-retention-days", 30),
    };
    let mut row = EntityValue::make(entity, RUN_ARTIFACT)?;
    row.set_sequenced_id_primary(entity)?;
    row.set("jobRunId", Value::string(&job_run_id));
    row.set("pipelineRunId", opt(field(run, "pipelineRunId")));
    row.set("repositoryId", opt(field(run, "repositoryId")));
    row.set("ownerId", opt(owner));
    row.set("artifactKind", Value::string(upload.kind.name()));
    row.set("name", Value::string(upload.name));
    row.set("path", opt(upload.path.map(str::to_string)));
    row.set("blobId", Value::string(&info.id));
    row.set("sizeBytes", int(i64::try_from(info.size).unwrap_or(i64::MAX)));
    row.set("sha256", Value::string(&info.id));
    row.set("cacheKey", opt(upload.cache_key.map(str::to_string)));
    row.set("protectedRef", yes_no(protected));
    row.set("keep", Value::string("N"));
    row.set(
        "expireDate",
        moqui_core::user::stamp_value(now.saturating_add(retention.saturating_mul(DAY_MILLIS))),
    );
    row.set("createdDate", moqui_core::user::stamp_value(now));
    if upload.kind == Kind::Cache {
        row.set("lastUsedDate", moqui_core::user::stamp_value(now));
    }
    row.create(entity)?;
    // two uploads at once may each have fit what was left: counted again with this one stored, the one that went past
    // a cap is taken back. Its content blob is left to gc#ContentBlobs, which deletes a blob no row references: blobs
    // are kept by their content, so the same bytes may be another upload's. The job's count is read again the same
    // way, so uploads sent at once do not each pass max-artifacts (SEC-GIT-90)
    let uploads = entity
        .find(RUN_ARTIFACT)?
        .eq("jobRunId", Value::string(&job_run_id))
        .count()?;
    if uploads > max {
        row.delete(entity)?;
        return Err(Error::new(format!(
            "a job uploads at most {max} artifacts, reports and caches"
        )));
    }
    for (setting, key, id, what, default) in &caps {
        let cap = cap_setting(setting, *default);
        let Some(id) = id.as_ref().filter(|_| cap > 0) else {
            continue;
        };
        let used = u64::try_from(stored_bytes(entity, key, id)?).unwrap_or(0);
        if used > cap {
            row.delete(entity)?;
            return Err(quota(what, used - info.size, cap, setting));
        }
    }
    if upload.kind == Kind::Report {
        // a report that does not parse is still a download
        if let Err(err) = record_failures(entity, &job_run_id, &blobs, &info) {
            tracing::error!(job = %job_run_id, "the failed tests of a report were not recorded: {err}");
        }
    }
    Ok(field(&row, "artifactId"))
}

/// Delete the caches kept under `key` `id`, least recently used first (by `lastUsedDate`, else when they were made),
/// until at most `target` bytes are kept or no cache is left. A cache is rebuilt by the next run that misses it; its
/// content blob is left to `gc#ContentBlobs`. The bytes kept after.
fn evict_caches(entity: &EntityFacade, key: &str, id: &str, used: u64, target: u64) -> Result<u64, Error> {
    let mut used = used;
    let caches = entity
        .find(RUN_ARTIFACT)?
        .eq(key, Value::string(id))
        .eq("artifactKind", Value::string("cache"))
        .order_by("lastUsedDate", false)
        .order_by("createdDate", false)
        .list()?;
    for cache in caches.iter() {
        if used <= target {
            break;
        }
        let size = u64::try_from(long(cache, "sizeBytes").unwrap_or(0)).unwrap_or(0);
        match cache.delete(entity) {
            Ok(()) => {
                tracing::info!(
                    "the cache {} of {key} {id} was evicted to make room under its upload cap",
                    field(cache, "cacheKey").unwrap_or_default()
                );
                used = used.saturating_sub(size);
            }
            // another upload evicted it first, or it could not go: the next one is tried
            Err(err) => tracing::warn!("a cache of {key} {id} was not evicted: {}", err.message()),
        }
    }
    Ok(used)
}

/// The refusal of an upload past a storage cap.
fn quota(what: &str, used: u64, cap: u64, setting: &str) -> Error {
    Error::new(format!(
        "GitCiStorageQuota: the {what} keeps {used} bytes of artifacts, reports and caches, and its cap is {cap} \
         (git-ci {setting})"
    ))
}

/// Give the artifacts stored before they carried their run's owner (`ownerId` empty) their run's, so an owner's cap
/// counts them: a batch at a time, once a process has none left. A failure is logged; the count goes on.
fn backfill_owners(entity: &EntityFacade) {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    const BATCH: i64 = 1000;
    // (rows read, rows given an owner): done when a batch was not full or filled nothing (runs with no owner)
    let filled = (|| -> Result<(usize, i64), Error> {
        let rows = entity
            .find(RUN_ARTIFACT)?
            .condition(Condition::is_null("ownerId"))
            .select_field("pipelineRunId")
            .limit(BATCH)
            .list()?;
        let runs: std::collections::BTreeSet<String> =
            rows.iter().filter_map(|row| field(row, "pipelineRunId")).collect();
        let mut given = 0;
        for run_id in &runs {
            let Some(owner) = super::one(entity, super::PIPELINE_RUN, "pipelineRunId", run_id)?
                .and_then(|run| field(&run, "ownerId"))
            else {
                continue;
            };
            given += entity
                .find(RUN_ARTIFACT)?
                .eq("pipelineRunId", Value::string(run_id))
                .condition(Condition::is_null("ownerId"))
                .update_all([("ownerId".to_string(), Value::string(owner))])?;
        }
        Ok((rows.len(), given))
    })();
    match filled {
        Ok((read, given)) if i64::try_from(read).unwrap_or(BATCH) < BATCH || given == 0 => {
            DONE.store(true, std::sync::atomic::Ordering::Release);
        }
        Ok(_) => {}
        Err(err) => tracing::error!("the owners of older CI artifacts were not filled in: {err}"),
    }
}

/// The bytes of the artifacts, reports and caches stored with `key` (`repositoryId` or `ownerId`) `id`.
pub(crate) fn stored_bytes(entity: &EntityFacade, key: &str, id: &str) -> Result<i64, Error> {
    Ok(entity
        .find(RUN_ARTIFACT)?
        .eq(key, Value::string(id))
        .select_field("sizeBytes")
        .list()?
        .iter()
        .map(|row| long(row, "sizeBytes").unwrap_or(0))
        .sum())
}

/// The bytes of each kind of upload stored with `key` `id`: (artifacts and reports, caches).
pub(crate) fn stored_by_kind(entity: &EntityFacade, key: &str, id: &str) -> Result<(i64, i64), Error> {
    let rows = entity
        .find(RUN_ARTIFACT)?
        .eq(key, Value::string(id))
        .select_field("sizeBytes")
        .select_field("artifactKind")
        .list()?;
    let (mut files, mut caches) = (0, 0);
    for row in rows.iter() {
        let size = long(row, "sizeBytes").unwrap_or(0);
        if field(row, "artifactKind").as_deref() == Some("cache") {
            caches += size;
        } else {
            files += size;
        }
    }
    Ok((files, caches))
}

/// The stored content of an artifact row, to read in ranges.
pub fn content(entity: &EntityFacade, artifact: &EntityValue) -> Result<Option<(BlobStore, BlobInfo)>, Error> {
    let _rows = crate::rows_guard();
    let Some(blob_id) = field(artifact, "blobId") else {
        return Ok(None);
    };
    let blobs = BlobStore::new(entity.clone());
    Ok(blobs.stat(&blob_id)?.map(|info| (blobs, info)))
}

/// Note that a cache was restored, so it is kept while it is used.
pub fn touch_cache(entity: &EntityFacade, artifact: &EntityValue) {
    let _rows = crate::rows_guard();
    let now = super::now_millis();
    let Some(id) = field(artifact, "artifactId") else {
        return;
    };
    let touched = super::set_fields(
        entity,
        RUN_ARTIFACT,
        "artifactId",
        &id,
        None,
        vec![
            ("lastUsedDate", moqui_core::user::stamp_value(now)),
            (
                "expireDate",
                moqui_core::user::stamp_value(
                    now.saturating_add(days("cache-retention-days", 30).saturating_mul(DAY_MILLIS)),
                ),
            ),
        ],
    );
    if let Err(err) = touched {
        tracing::warn!(artifact = %id, "a cache's use was not recorded: {err}");
    }
}

/// What the retention removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Expired {
    pub artifacts: i64,
    pub logs: i64,
    pub chunks: i64,
}

/// The daily retention, as of `now` (milliseconds): artifacts, reports and caches past their `expireDate` that are
/// not kept; the logs of jobs that ended more than the conf's `git-ci` `log-retention-days` (90) ago, with their step
/// and failure rows left in place; and the log chunks an ended job left (a server that stopped between a job's end
/// and its log's blob). The content blobs go with `gc#ContentBlobs`.
pub fn expire(entity: &EntityFacade, now: i64) -> Result<Expired, Error> {
    let _rows = crate::rows_guard();
    let mut expired = Expired::default();
    let old = entity
        .find(RUN_ARTIFACT)?
        .condition(Condition::less("expireDate", moqui_core::user::stamp_value(now)))
        .list()?;
    for row in old.iter().filter(|row| !is_yes(row, "keep")) {
        // one row that does not delete does not stop the rest
        match row.delete(entity) {
            Ok(()) => expired.artifacts += 1,
            Err(err) => tracing::error!("an expired artifact was not removed: {err}"),
        }
    }
    let log_cutoff = now.saturating_sub(days("log-retention-days", 90).saturating_mul(DAY_MILLIS));
    let jobs = entity
        .find(JOB_RUN)?
        .condition(Condition::less(
            "finishedDate",
            moqui_core::user::stamp_value(log_cutoff),
        ))
        .condition(Condition::is_not_null("logBlobId"))
        .list()?;
    for job in jobs.iter() {
        let Some(id) = field(job, "jobRunId") else { continue };
        match super::set_fields(entity, JOB_RUN, "jobRunId", &id, None, vec![("logBlobId", Value::Null)]) {
            Ok(_) => expired.logs += 1,
            Err(err) => tracing::error!(job = %id, "an expired log was not removed: {err}"),
        }
    }
    // chunks of jobs that ended a while ago: their log was never made a blob
    let stale = now.saturating_sub(60 * 60 * 1000);
    let chunk_jobs = entity
        .find(LOG_CHUNK)?
        .condition(Condition::less("createdDate", moqui_core::user::stamp_value(stale)))
        .select_field("jobRunId")
        .limit(10_000)
        .list()?;
    let mut seen = std::collections::BTreeSet::new();
    for chunk in chunk_jobs.iter() {
        let Some(id) = field(chunk, "jobRunId") else { continue };
        if !seen.insert(id.clone()) {
            continue;
        }
        let ended = super::one(entity, JOB_RUN, "jobRunId", &id)?.is_none_or(|job| {
            field(&job, "statusId").is_some_and(|status| super::ENDED.contains(&status.as_str()))
                && millis(&job, "finishedDate").is_none_or(|finished| finished < stale)
        });
        if ended {
            match super::log::finalize(entity, &id) {
                Ok(()) => expired.chunks += 1,
                Err(err) => tracing::error!(job = %id, "the log chunks an ended job left were not collected: {err}"),
            }
        }
    }
    Ok(expired)
}
