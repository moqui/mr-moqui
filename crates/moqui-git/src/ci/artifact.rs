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
//! for protected refs: a run for a protected ref reads only caches such a run wrote, and an untrusted run writes
//! none. The daily retention removes what expired; `gc#ContentBlobs` then deletes the blobs.

use std::io::Read;

use moqui_lib::object::Value;

use moqui_core::context::Error;
use moqui_core::entity::{Condition, EntityFacade, EntityValue};
use moqui_core::resource::blob::{BlobInfo, BlobStore, Expected};

use super::{field, int, is_yes, long, millis, opt, yes_no, JOB_RUN, LOG_CHUNK, RUN_ARTIFACT, TEST_FAILURE};

const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;
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
}

fn days(name: &str, default: u64) -> i64 {
    i64::try_from(super::number_setting(name, default)).unwrap_or(i64::MAX / DAY_MILLIS)
}

/// The cache a run reads for `key`: one a run of its own class wrote, and for a run that is not for a protected ref,
/// one a protected run wrote when there is no other.
pub fn find_cache(entity: &EntityFacade, run: &EntityValue, key: &str) -> Result<Option<EntityValue>, Error> {
    let _rows = crate::rows_guard();
    let protected = is_yes(run, "protectedRef");
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
    let protected = is_yes(run, "protectedRef");
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
    } else {
        let max = i64::try_from(super::number_setting("max-artifacts", 100)).unwrap_or(100);
        let count = entity
            .find(RUN_ARTIFACT)?
            .eq("jobRunId", Value::string(&job_run_id))
            .count()?;
        if count >= max {
            return Err(Error::new(format!("a job uploads at most {max} artifacts and reports")));
        }
    }
    let blobs = BlobStore::new(entity.clone());
    let info = blobs.put(input, &Expected::default(), upload.kind.max_bytes())?;
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
    if upload.kind == Kind::Report {
        // a report that does not parse is still a download
        if let Err(err) = record_failures(entity, &job_run_id, &blobs, &info) {
            tracing::error!(job = %job_run_id, "the failed tests of a report were not recorded: {err}");
        }
    }
    Ok(field(&row, "artifactId"))
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
