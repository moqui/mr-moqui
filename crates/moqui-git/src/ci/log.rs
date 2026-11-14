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

//! A job's log (docs/interfaces/Pipelines.md "Logs"). While the job runs its runner appends lines, kept as
//! `GitRunLogChunk` rows with the job's secrets masked as they are stored. When the job ends the chunks become one
//! content blob (`GitJobRun.logBlobId`) and are removed. A page tails a running job by chunk sequence and reads an
//! ended one by byte offset; the two meet, since each chunk carries the offset it starts at.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use moqui_lib::object::Value;

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::resource::blob::{BlobStore, Expected};

use super::{field, int, long, trust, JOB_RUN, JOB_STEP, LOG_CHUNK, PIPELINE_RUN};

/// The notification topic of new lines of a job's log: `{jobRunId, nextChunkSeq, finished}` to the users who read
/// the log with `watch` in the last minutes.
pub const LOG_TOPIC: &str = "GitCiJobLog";

/// The most text of one chunk row.
const CHUNK_BYTES: usize = 64 * 1024;
/// A job's log stops growing here unless the conf's `git-ci` `max-log-bytes` says otherwise.
const DEFAULT_MAX_LOG_BYTES: u64 = 16 * 1024 * 1024;
/// A watcher who has not read for this long is told no more.
const WATCH_MILLIS: i64 = 3 * 60 * 1000;
/// Watchers of a job are told at most this often.
const NOTIFY_MILLIS: i64 = 1000;

/// Who watches a job's log: each user's last read, and when they were last told.
#[derive(Default)]
struct Watchers {
    read: HashMap<String, i64>,
    told: i64,
}

/// Per factory: the mask patterns of running jobs (decrypted once a job), and who watches which log, by job run.
#[derive(Default)]
struct LogState {
    masks: Mutex<HashMap<String, Arc<Vec<String>>>>,
    watchers: Mutex<HashMap<String, Watchers>>,
}

fn state() -> Option<Arc<LogState>> {
    ExecutionContext::with_current(|ec| ec.factory().extension(|_| LogState::default()))
}

/// What to mask in the log of `job`: the values of the secrets it is given.
fn patterns(entity: &EntityFacade, job: &EntityValue) -> Arc<Vec<String>> {
    let id = field(job, "jobRunId").unwrap_or_default();
    let state = state();
    if let Some(known) = state
        .as_ref()
        .and_then(|state| moqui_lib::sync::lock(&state.masks).get(&id).cloned())
    {
        return known;
    }
    let made = (|| -> Result<Vec<String>, Error> {
        let Some(run) = field(job, "pipelineRunId")
            .map(|run| super::one(entity, PIPELINE_RUN, "pipelineRunId", &run))
            .transpose()?
            .flatten()
        else {
            return Ok(Vec::new());
        };
        let spec: serde_json::Value =
            serde_json::from_str(&field(job, "jobSpec").unwrap_or_default()).unwrap_or(serde_json::Value::Null);
        let values: Vec<String> = trust::secrets_for(entity, &run, &spec, false)?
            .into_iter()
            .map(|given| given.value)
            .collect();
        Ok(trust::mask_patterns(&values))
    })();
    let made = Arc::new(made.unwrap_or_else(|err| {
        tracing::error!(job = %id, "the secrets to mask in a job's log were not read: {err}");
        Vec::new()
    }));
    if let Some(state) = state {
        moqui_lib::sync::lock(&state.masks).insert(id, Arc::clone(&made));
    }
    made
}

/// Forget what is kept in memory for a job that ended.
pub(crate) fn forget(job_run_id: &str) {
    if let Some(state) = state() {
        moqui_lib::sync::lock(&state.masks).remove(job_run_id);
        moqui_lib::sync::lock(&state.watchers).remove(job_run_id);
    }
}

/// Note that the current user reads the log of a job and wants to be told of new lines ([`LOG_TOPIC`]).
pub(crate) fn watch(job_run_id: &str) {
    let (Some(state), Some(user)) = (state(), moqui_core::user::current_user_id().ok().flatten()) else {
        return;
    };
    let now = super::now_millis();
    let mut watchers = moqui_lib::sync::lock(&state.watchers);
    let entry = watchers.entry(job_run_id.to_string()).or_default();
    entry.read.retain(|_, read| now - *read < WATCH_MILLIS);
    entry.read.insert(user, now);
}

/// Tell the watchers of a job that its log grew, or ended.
fn notify(job_run_id: &str, next_seq: i64, finished: bool) {
    let Some(state) = state() else { return };
    let now = super::now_millis();
    let users: Vec<String> = {
        let mut watchers = moqui_lib::sync::lock(&state.watchers);
        let Some(entry) = watchers.get_mut(job_run_id) else {
            return;
        };
        if !finished && now - entry.told < NOTIFY_MILLIS {
            return;
        }
        entry.told = now;
        entry.read.retain(|_, read| now - *read < WATCH_MILLIS);
        entry.read.keys().cloned().collect()
    };
    if users.is_empty() {
        return;
    }
    let mut message = moqui_lib::object::ValueMap::default();
    message.insert("jobRunId".to_string(), Value::string(job_run_id));
    message.insert("nextChunkSeq".to_string(), int(next_seq));
    message.insert("finished".to_string(), Value::Bool(finished));
    let sent = moqui_core::notify::NotificationMessage::new()
        .topic(LOG_TOPIC)
        .sub_topic(job_run_id)
        .user_ids(users)
        .message_map(message)
        .persist(false)
        .show_alert(false)
        .send();
    if let Err(err) = sent {
        tracing::debug!(job = %job_run_id, "a job log notification was not sent: {err}");
    }
}

/// Split `text` into pieces of at most [`CHUNK_BYTES`], at line ends where there are any.
fn pieces(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while rest.len() > CHUNK_BYTES {
        let mut cut = CHUNK_BYTES;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        let (head, _) = rest.split_at(cut);
        let cut = head.rfind('\n').map_or(cut, |at| at + 1);
        let (head, tail) = rest.split_at(cut);
        out.push(head);
        rest = tail;
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

/// Append `text` (whole lines) to the log of the running job `job`, for its step `step_seq`, masked. The sequence
/// the next chunk takes. Past the size limit the text is dropped, with one line that says so.
pub fn append(entity: &EntityFacade, job: &EntityValue, step_seq: Option<i64>, text: &str) -> Result<i64, Error> {
    let _rows = crate::rows_guard();
    let job_run_id = field(job, "jobRunId").unwrap_or_default();
    let mut next = long(job, "nextChunkSeq").unwrap_or(1).max(1);
    let mut bytes = long(job, "logBytes").unwrap_or(0).max(0);
    if text.is_empty() {
        return Ok(next);
    }
    let max = i64::try_from(super::number_setting("max-log-bytes", DEFAULT_MAX_LOG_BYTES)).unwrap_or(i64::MAX);
    if bytes >= max {
        return Ok(next);
    }
    let masked = trust::mask(text, &patterns(entity, job));
    let room = usize::try_from(max - bytes).unwrap_or(usize::MAX);
    let mut kept = masked.as_str();
    let mut truncated = false;
    if kept.len() > room {
        let mut cut = room;
        while !kept.is_char_boundary(cut) {
            cut -= 1;
        }
        kept = &kept[..cut];
        truncated = true;
    }
    let notice = format!("\n[the log is over {max} bytes; the rest is not kept]\n");
    let mut parts = pieces(kept);
    if truncated {
        parts.push(&notice);
    }
    let first = next;
    for part in parts {
        let mut row = EntityValue::make(entity, LOG_CHUNK)?;
        row.set("jobRunId", Value::string(&job_run_id));
        row.set("chunkSeq", int(next));
        row.set("stepSeq", step_seq.map_or(Value::Null, int));
        row.set("byteOffset", int(bytes));
        row.set("chunkText", Value::string(part));
        row.set("createdDate", moqui_core::user::now_value());
        row.create(entity)?;
        next += 1;
        bytes += i64::try_from(part.len()).unwrap_or(i64::MAX);
    }
    if truncated {
        bytes = bytes.max(max);
    }
    super::set_fields(
        entity,
        JOB_RUN,
        "jobRunId",
        &job_run_id,
        None,
        vec![("nextChunkSeq", int(next)), ("logBytes", int(bytes))],
    )?;
    if let Some(step_seq) = step_seq {
        // the first chunk of the step, once
        let step = entity
            .find(JOB_STEP)?
            .eq("jobRunId", Value::string(&job_run_id))
            .eq("stepSeq", int(step_seq))
            .one()?;
        if let Some(mut step) = step.filter(|step| long(step, "firstChunkSeq").is_none()) {
            step.set("firstChunkSeq", int(first));
            step.update(entity)?;
        }
    }
    notify(&job_run_id, next, false);
    Ok(next)
}

/// Make the log of an ended job one content blob and remove its chunks. A job with no log lines gets no blob.
pub fn finalize(entity: &EntityFacade, job_run_id: &str) -> Result<(), Error> {
    let _rows = crate::rows_guard();
    let chunks = entity
        .find(LOG_CHUNK)?
        .eq("jobRunId", Value::string(job_run_id))
        .order_by("chunkSeq", false)
        .list()?;
    if !chunks.is_empty() {
        let blobs = BlobStore::new(entity.clone());
        let mut writer = blobs.begin()?;
        for chunk in chunks.iter() {
            writer.write(field_text(chunk).as_bytes())?;
        }
        let info = writer.finish(&Expected::default())?;
        super::set_fields(
            entity,
            JOB_RUN,
            "jobRunId",
            job_run_id,
            None,
            vec![
                ("logBlobId", Value::string(&info.id)),
                ("logBytes", int(i64::try_from(info.size).unwrap_or(i64::MAX))),
            ],
        )?;
        entity
            .find(LOG_CHUNK)?
            .eq("jobRunId", Value::string(job_run_id))
            .delete_all()?;
    }
    let next = super::one(entity, JOB_RUN, "jobRunId", job_run_id)?
        .and_then(|job| long(&job, "nextChunkSeq"))
        .unwrap_or(1);
    notify(job_run_id, next, true);
    forget(job_run_id);
    Ok(())
}

/// A chunk's text as stored: not trimmed, so the lines of the log keep their ends.
fn field_text(chunk: &EntityValue) -> String {
    chunk
        .get("chunkText")
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

/// A piece of a job's log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogRead {
    /// (chunkSeq, stepSeq, text) of the chunks after `after_seq`, while the job runs.
    pub chunks: Vec<(i64, Option<i64>, String)>,
    pub next_chunk_seq: i64,
    /// The stored log from `offset`, once the job has ended.
    pub text: String,
    /// The byte after what was returned: pass it as `offset` once the job has ended.
    pub next_offset: i64,
    pub log_bytes: i64,
    pub finished: bool,
}

/// Read the log of `job`: while it runs, up to `limit` chunks after `after_seq`; once it has ended, up to `max_bytes`
/// of the stored log from `offset`.
pub fn read(
    entity: &EntityFacade,
    job: &EntityValue,
    after_seq: i64,
    limit: usize,
    offset: i64,
    max_bytes: usize,
) -> Result<LogRead, Error> {
    let _rows = crate::rows_guard();
    let job_run_id = field(job, "jobRunId").unwrap_or_default();
    let finished = field(job, "statusId").is_some_and(|status| super::ENDED.contains(&status.as_str()));
    let log_bytes = long(job, "logBytes").unwrap_or(0);
    let mut out = LogRead {
        next_chunk_seq: after_seq.max(0) + 1,
        next_offset: offset.max(0),
        log_bytes,
        finished,
        ..LogRead::default()
    };
    if let Some(blob_id) = field(job, "logBlobId") {
        let blobs = BlobStore::new(entity.clone());
        if let Some(info) = blobs.stat(&blob_id)? {
            let from = u64::try_from(offset.max(0)).unwrap_or(0).min(info.size);
            let bytes = blobs.read_range(&info, from, max_bytes)?;
            out.next_offset = i64::try_from(from).unwrap_or(0) + i64::try_from(bytes.len()).unwrap_or(0);
            out.text = String::from_utf8_lossy(&bytes).into_owned();
        }
        out.next_chunk_seq = long(job, "nextChunkSeq").unwrap_or(1);
        return Ok(out);
    }
    let chunks = entity
        .find(LOG_CHUNK)?
        .eq("jobRunId", Value::string(&job_run_id))
        .condition(moqui_core::entity::Condition::greater(
            "chunkSeq",
            int(after_seq.max(0)),
        ))
        .order_by("chunkSeq", false)
        .limit(i64::try_from(limit.max(1)).unwrap_or(100))
        .list()?;
    for chunk in chunks.iter() {
        let seq = long(chunk, "chunkSeq").unwrap_or(0);
        let text = field_text(chunk);
        out.next_chunk_seq = seq + 1;
        out.next_offset = long(chunk, "byteOffset").unwrap_or(0) + i64::try_from(text.len()).unwrap_or(0);
        out.chunks.push((seq, long(chunk, "stepSeq"), text));
    }
    Ok(out)
}
