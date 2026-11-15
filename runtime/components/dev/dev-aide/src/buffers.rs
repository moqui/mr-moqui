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

//! The unsaved buffers of the visual views (plans/design-dev-aide.md F14, D14): by user and buffer id, the texts of
//! files at their locations, for two hours since their last put. A Design frame's canvas buffer
//! (`put#CanvasBuffer`, `crate::screens`) holds the texts its frame renders; a guided editor's edit buffer (the
//! terminal's guided editors, `Entities/EditTui`, `Services/EditTui` and `Screens/DesignTui`) holds the files its
//! edits are staged in, each with the hash of the disk text it started from, until Save writes them in kind order
//! (entities, services, ECA rules, screens, data, as the browser's Save all) or Discard drops them. The screen
//! overlay's provider answers a `buffer:<id>` token from either kind, so a preview renders the staged text.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use moqui_core::context::{Error, ExecutionContext, ExecutionContextFactory};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_core::screen::overlay::Overlay;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use serde_json::{json, Value as Json};

use crate::tier::Tier;
use crate::workspace::{flag, json_value, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(get_edit_buffer), get_edit_buffer)?;
    registry.service_function(moqui_core::service_location!(put_edit_buffer), put_edit_buffer)?;
    registry.service_function(
        moqui_core::service_location!(apply_edit_buffer_ops),
        apply_edit_buffer_ops,
    )?;
    registry.service_function(moqui_core::service_location!(save_edit_buffer), save_edit_buffer)?;
    registry.service_function(moqui_core::service_location!(clear_edit_buffer), clear_edit_buffer)
}

/// How long a buffer is kept since its last put.
pub(crate) const BUFFER_LIFE: Duration = Duration::from_secs(2 * 60 * 60);
/// The most buffers kept for all users; the oldest goes first.
const MAX_BUFFERS: usize = 200;
/// How long a buffer that expired is remembered, so a guided editor can say its edits are gone.
const EXPIRED_MEMORY: Duration = Duration::from_secs(24 * 60 * 60);
/// The most files one buffer holds.
const MAX_FILES: usize = 20;

/// One file of a buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BufferFile {
    pub root_id: String,
    /// The path in the root.
    pub path: String,
    /// The `component://` location (else the full path) the overlay puts the text at.
    pub location: String,
    pub text: String,
    /// The hash of the disk text the edits started from; empty for a file not there yet.
    pub base_hash: String,
    /// A data file whose records load after Save (a seed companion).
    pub load: bool,
}

struct Buffer {
    at: Instant,
    files: Vec<BufferFile>,
}

/// The buffers, by user and buffer id, and the ids of those that expired.
#[derive(Default)]
pub(crate) struct Store {
    buffers: HashMap<(String, String), Buffer>,
    expired: HashMap<(String, String), Instant>,
}

impl Store {
    /// Drop the buffers past their life as of `now`, remembering their ids.
    fn expire(&mut self, now: Instant) {
        let gone: Vec<(String, String)> = self
            .buffers
            .iter()
            .filter(|(_, buffer)| now.saturating_duration_since(buffer.at) >= BUFFER_LIFE)
            .map(|(key, _)| key.clone())
            .collect();
        for key in gone {
            self.buffers.remove(&key);
            self.expired.insert(key, now);
        }
        self.expired
            .retain(|_, at| now.saturating_duration_since(*at) < EXPIRED_MEMORY);
        while self.expired.len() > MAX_BUFFERS {
            let Some(oldest) = self
                .expired
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.expired.remove(&oldest);
        }
    }

    /// The files of a buffer as of `now`, none when it is not kept.
    pub(crate) fn files(&mut self, key: &(String, String), now: Instant) -> Option<&[BufferFile]> {
        self.expire(now);
        self.buffers.get(key).map(|buffer| buffer.files.as_slice())
    }

    /// Whether the buffer expired with edits in it (and was not put again since).
    pub(crate) fn was_expired(&mut self, key: &(String, String), now: Instant) -> bool {
        self.expire(now);
        self.expired.contains_key(key)
    }

    /// When a kept buffer expires.
    pub(crate) fn expires(&self, key: &(String, String)) -> Option<Instant> {
        self.buffers.get(key).map(|buffer| buffer.at + BUFFER_LIFE)
    }

    /// Replace all of a buffer's files (a canvas buffer).
    pub(crate) fn replace(&mut self, key: (String, String), files: Vec<BufferFile>, now: Instant) {
        self.expire(now);
        self.make_room(&key);
        self.expired.remove(&key);
        self.buffers.insert(key, Buffer { at: now, files });
    }

    /// Put one file in a buffer, in place of the one of its root and path; `None` as the text takes it out (its text
    /// is the disk's again). A buffer left with no files is dropped.
    pub(crate) fn put(
        &mut self,
        key: (String, String),
        file: BufferFile,
        keep: bool,
        now: Instant,
    ) -> Result<(), LibError> {
        self.expire(now);
        self.expired.remove(&key);
        if !keep {
            self.remove(&key, Some((&file.root_id, &file.path)));
            return Ok(());
        }
        self.make_room(&key);
        let buffer = self.buffers.entry(key).or_insert_with(|| Buffer {
            at: now,
            files: Vec::new(),
        });
        buffer.at = now;
        let count = buffer.files.len();
        match buffer
            .files
            .iter_mut()
            .find(|have| have.root_id == file.root_id && have.path == file.path)
        {
            Some(have) => {
                // a seed file marked to load stays marked
                let load = have.load || file.load;
                *have = file;
                have.load = load;
            }
            None if count >= MAX_FILES => {
                return Err(LibError::new(format!(
                    "a buffer holds at most {MAX_FILES} files: save or discard first"
                )))
            }
            None => buffer.files.push(file),
        }
        Ok(())
    }

    /// Take a file out of a buffer, or the whole buffer with `None`.
    pub(crate) fn remove(&mut self, key: &(String, String), file: Option<(&str, &str)>) {
        let empty = match (file, self.buffers.get_mut(key)) {
            (Some((root_id, path)), Some(buffer)) => {
                buffer
                    .files
                    .retain(|have| !(have.root_id == root_id && have.path == path));
                buffer.files.is_empty()
            }
            (None, Some(_)) => true,
            (_, None) => false,
        };
        if empty {
            self.buffers.remove(key);
        }
        if file.is_none() {
            self.expired.remove(key);
        }
    }

    fn make_room(&mut self, key: &(String, String)) {
        if self.buffers.contains_key(key) || self.buffers.len() < MAX_BUFFERS {
            return;
        }
        if let Some(oldest) = self
            .buffers
            .iter()
            .min_by_key(|(_, buffer)| buffer.at)
            .map(|(key, _)| key.clone())
        {
            self.buffers.remove(&oldest);
        }
    }
}

pub(crate) fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Store::default()))
}

/// A buffer id from the parameters: `bufferId`, letters, digits and `-`, or the guided id of `target`.
pub(crate) fn buffer_id(parameters: &Parameters) -> Result<String, LibError> {
    let target = text(parameters, "target");
    let id = match text(parameters, "bufferId") {
        id if id.is_empty() && !target.is_empty() => guided_id(&target),
        id => id,
    };
    if id.is_empty() || id.len() > 64 || !id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-') {
        return Err(LibError::new("a buffer id is letters, digits and -"));
    }
    Ok(id)
}

/// The edit buffer of a guided editor's target (`service:<name>`, `entity:<name>`, `screen:<location>`): one staged
/// edit per user and artifact.
pub(crate) fn guided_id(target: &str) -> String {
    let hash = crate::workspace::sha256(target.as_bytes());
    format!("guided-{}", hash.get(..20).unwrap_or(&hash))
}

/// The order Save writes a buffer's files in (5.7, 6.2; the browser's `pure.kindRank`): entities, then services,
/// then ECA rules, then screens, then data and the rest, so a screen never loads before its field.
pub(crate) fn kind_rank(path: &str) -> u8 {
    let under = |dir: &str| path.starts_with(&format!("{dir}/")) || path.contains(&format!("/{dir}/"));
    let rules = [".secas.xml", ".eecas.xml", ".emecas.xml", ".gitecas.xml"];
    if under("entity") && !path.ends_with(".eecas.xml") {
        0
    } else if rules.iter().any(|suffix| path.ends_with(suffix)) {
        2
    } else if under("service") {
        1
    } else if under("screen") || under("screen-extend") {
        3
    } else {
        4
    }
}

/// The files in the order Save writes them: by kind, else as they were put.
pub(crate) fn save_order(files: &[BufferFile]) -> Vec<&BufferFile> {
    let mut ordered: Vec<&BufferFile> = files.iter().collect();
    ordered.sort_by_key(|file| kind_rank(&file.path));
    ordered
}

/// What writing one file came to.
pub(crate) enum Written {
    Saved(Parameters),
    /// The disk changed since the edits started: its hash and text.
    Conflict {
        disk_hash: String,
        disk_text: String,
    },
}

/// What saving a buffer came to: the files written (each with what its save answered), and the one that stopped it.
pub(crate) struct Saving<'a> {
    pub saved: Vec<(&'a BufferFile, Parameters)>,
    pub conflict: Option<(&'a BufferFile, String, String)>,
    pub failed: Option<(&'a BufferFile, LibError)>,
}

/// Write the files in kind order through `write`, stopping at the first conflict or failure: a later file may need
/// an earlier one (a screen its field), so nothing after it is written.
pub(crate) fn save_in_order<'a>(
    files: &'a [BufferFile],
    mut write: impl FnMut(&BufferFile) -> Result<Written, LibError>,
) -> Saving<'a> {
    let mut saving = Saving {
        saved: Vec::new(),
        conflict: None,
        failed: None,
    };
    for file in save_order(files) {
        match write(file) {
            Ok(Written::Saved(out)) => saving.saved.push((file, out)),
            Ok(Written::Conflict { disk_hash, disk_text }) => {
                saving.conflict = Some((file, disk_hash, disk_text));
                break;
            }
            Err(err) => {
                saving.failed = Some((file, err));
                break;
            }
        }
    }
    saving
}

fn user() -> Result<String, LibError> {
    Ok(moqui_core::user::current_user_id()?.unwrap_or_default())
}

/// A list parameter, given as a list or as its JSON text.
pub(crate) fn json_list(parameters: &Parameters, name: &str) -> Vec<Json> {
    match parameters.get(name) {
        Some(Value::String(raw)) if raw.trim().is_empty() => Vec::new(),
        Some(Value::String(raw)) => serde_json::from_str(raw).unwrap_or_default(),
        Some(value) => match moqui_lib::json::to_json(value) {
            Json::Array(items) => items,
            Json::Null => Vec::new(),
            other => vec![other],
        },
        None => Vec::new(),
    }
}

/// A file of a root as the views read it: its text and hash on disk, or `None` when it is not there.
fn disk(context: &Context, root_id: &str, path: &str) -> Result<Option<(String, String)>, LibError> {
    let root = context.root(root_id)?;
    let resolved = context.resolve(root, path)?;
    match std::fs::read(&resolved.path) {
        Ok(bytes) => Ok(Some((
            String::from_utf8_lossy(&bytes)
                .trim_start_matches('\u{feff}')
                .to_string(),
            crate::workspace::sha256(&bytes),
        ))),
        Err(_) => Ok(None),
    }
}

/// The overlay location of a file of a root.
fn location(context: &Context, root_id: &str, path: &str) -> Result<String, LibError> {
    let root = context.root(root_id)?;
    let resolved = context.resolve(root, path)?;
    Ok(crate::screens::location_of(root, &resolved.relative).unwrap_or_else(|| resolved.path.display().to_string()))
}

/// The pending summary of a buffer: its files and when it expires.
fn summary(store: &Store, key: &(String, String), files: &[BufferFile], now: Instant) -> Json {
    let minutes = store
        .expires(key)
        .map(|at| at.saturating_duration_since(now).as_secs().div_ceil(60));
    json!({
        "count": files.len(),
        "files": files.iter().map(|file| json!({ "rootId": file.root_id, "path": file.path })).collect::<Vec<_>>(),
        "names": files.iter().map(|file| file.path.rsplit('/').next().unwrap_or(&file.path).to_string()).collect::<Vec<_>>(),
        "expiresInMinutes": minutes,
    })
}

/// `get#EditBuffer`: a guided editor's buffer (`bufferId`, or `target` for its guided id): what it holds
/// (`pending`: the files, how many, when it expires), whether it expired with edits in it, and for each of `files`
/// (`[{rootId, path, initialText}]`) its text as staged, else the disk's, else the initial text of a file made on its
/// first save; with `outline` each one's outline, with `diff` the unified diff of a staged one against the disk.
fn get_edit_buffer(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let id = buffer_id(parameters)?;
    let key = (user()?, id.clone());
    let now = Instant::now();
    let (staged, expired, pending) = {
        let mut kept = lock(store());
        let staged: Vec<BufferFile> = kept.files(&key, now).map(<[BufferFile]>::to_vec).unwrap_or_default();
        let expired = staged.is_empty() && kept.was_expired(&key, now);
        let pending = summary(&kept, &key, &staged, now);
        (staged, expired, pending)
    };
    let with_outline = flag(parameters, "outline");
    let with_diff = flag(parameters, "diff");
    let mut answered = Vec::new();
    for asked in json_list(parameters, "files").iter().take(MAX_FILES) {
        let root_id = asked["rootId"].as_str().unwrap_or_default();
        let path = asked["path"].as_str().unwrap_or_default();
        if root_id.is_empty() || path.is_empty() {
            continue;
        }
        let root = context.root(root_id)?;
        let relative = context.resolve(root, path)?.relative;
        let on_disk = disk(&context, root_id, &relative)?;
        let mine = staged
            .iter()
            .find(|file| file.root_id == root_id && file.path == relative);
        let (body, base_hash) = match (mine, &on_disk) {
            (Some(file), _) => (file.text.clone(), file.base_hash.clone()),
            (None, Some((body, hash))) => (body.clone(), hash.clone()),
            (None, None) => (
                asked["initialText"].as_str().unwrap_or_default().to_string(),
                String::new(),
            ),
        };
        let mut out = json!({
            "rootId": root_id, "path": relative, "text": body, "baseHash": base_hash,
            "staged": mine.is_some(), "missing": on_disk.is_none(),
            "changedOnDisk": mine.is_some() && on_disk.as_ref().map_or(String::new(), |(_, hash)| hash.clone()) != base_hash,
        });
        if let Err(err) = context.may_write(root, &relative) {
            out["readOnly"] = json!(true);
            out["readOnlyReason"] = json!(err.to_string());
        }
        if with_outline {
            out["outline"] = match moqui_lib::xml_edit::outline(&body) {
                Ok(outline) => serde_json::to_value(outline).unwrap_or(Json::Null),
                Err(err) => {
                    out["outlineError"] = json!(err.to_string());
                    Json::Null
                }
            };
        }
        if with_diff && mine.is_some() {
            let before = on_disk.as_ref().map_or("", |(body, _)| body.as_str());
            out["diff"] = json!(moqui_core::resource::textdiff::unified(
                before,
                &body,
                &format!("{relative} (disk)"),
                &format!("{relative} (staged)")
            ));
        }
        answered.push(out);
    }
    let mut out = Parameters::new();
    out.insert("bufferId".into(), Value::string(id));
    out.insert("files".into(), json_value(&answered));
    out.insert("pending".into(), json_value(&pending));
    out.insert("expired".into(), Value::Bool(expired));
    Ok(out)
}

/// Stage `body` for a file of a root: refused when the user may not write it; taken out of the buffer when it is the
/// disk's text again.
fn stage(
    context: &Context,
    key: (String, String),
    root_id: &str,
    path: &str,
    body: String,
    base_hash: String,
    load: bool,
) -> Result<(), LibError> {
    let root = context.root(root_id)?;
    let relative = context.resolve(root, path)?.relative;
    context.may_write(root, &relative)?;
    if body.len() as u64 > context.settings.max_edit_bytes {
        return Err(LibError::new(format!("{relative} is over the edit limit")));
    }
    let unchanged =
        disk(context, root_id, &relative)?.is_some_and(|(disk_text, hash)| hash == base_hash && disk_text == body);
    let file = BufferFile {
        location: location(context, root_id, &relative)?,
        root_id: root_id.to_string(),
        path: relative,
        text: body,
        base_hash,
        load,
    };
    lock(store()).put(key, file, !unchanged, Instant::now())
}

/// `put#EditBuffer`: stage texts in a guided editor's buffer (`files`, `[{rootId, path, text, baseHash, load}]`, the
/// user's own files to write); nothing is written until Save.
fn put_edit_buffer(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let id = buffer_id(parameters)?;
    let key = (user()?, id.clone());
    for file in json_list(parameters, "files").iter().take(MAX_FILES) {
        stage(
            &context,
            key.clone(),
            file["rootId"].as_str().unwrap_or_default(),
            file["path"].as_str().unwrap_or_default(),
            file["text"].as_str().unwrap_or_default().to_string(),
            file["baseHash"].as_str().unwrap_or_default().to_string(),
            file["load"].as_bool().unwrap_or(false),
        )?;
    }
    pending_out(&key, id)
}

fn pending_out(key: &(String, String), id: String) -> Result<Parameters, LibError> {
    let now = Instant::now();
    let mut kept = lock(store());
    let files: Vec<BufferFile> = kept.files(key, now).map(<[BufferFile]>::to_vec).unwrap_or_default();
    let pending = summary(&kept, key, &files, now);
    let mut out = Parameters::new();
    out.insert("bufferId".into(), Value::string(id));
    out.insert("pending".into(), json_value(&pending));
    Ok(out)
}

/// `apply#EditBufferOps`: the operations of the lossless XML layer (`ops`) on one file of a guided editor's buffer
/// (`rootId`, `path`): on its staged text, else the disk's, else `initialText` for a file made on its first save; the
/// result staged. With `load` the file is a data file whose records load after Save. Answers the new text and outline.
fn apply_edit_buffer_ops(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let id = buffer_id(parameters)?;
    let key = (user()?, id.clone());
    let root_id = text(parameters, "rootId");
    let root = context.root(&root_id)?;
    let relative = context.resolve(root, &text(parameters, "path"))?.relative;
    context.may_write(root, &relative)?;
    let staged = lock(store()).files(&key, Instant::now()).and_then(|files| {
        files
            .iter()
            .find(|file| file.root_id == root_id && file.path == relative)
            .map(|file| (file.text.clone(), file.base_hash.clone()))
    });
    let (body, base_hash) = match staged {
        Some(found) => found,
        None => {
            disk(&context, &root_id, &relative)?.unwrap_or_else(|| (text(parameters, "initialText"), String::new()))
        }
    };
    let ops: Vec<moqui_lib::xml_edit::Op> = serde_json::from_value(Json::Array(json_list(parameters, "ops")))
        .map_err(|err| LibError::new(format!("ops: {err}")))?;
    let applied = moqui_lib::xml_edit::apply(&body, &ops)?;
    let outline = moqui_lib::xml_edit::outline(&applied.text)?;
    stage(
        &context,
        key.clone(),
        &root_id,
        &relative,
        applied.text.clone(),
        base_hash,
        flag(parameters, "load"),
    )?;
    let mut out = pending_out(&key, id)?;
    out.insert("text".into(), Value::string(applied.text));
    out.insert("outline".into(), json_value(&outline));
    Ok(out)
}

/// `clear#EditBuffer`: Discard: drop a guided editor's buffer, or one file of it (`rootId`, `path`: take the disk's).
fn clear_edit_buffer(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let id = buffer_id(parameters)?;
    let key = (user()?, id.clone());
    let root_id = text(parameters, "rootId");
    let path = text(parameters, "path");
    if root_id.is_empty() {
        lock(store()).remove(&key, None);
    } else {
        let root = context.root(&root_id)?;
        let relative = context.resolve(root, &path)?.relative;
        lock(store()).remove(&key, Some((&root_id, &relative)));
    }
    pending_out(&key, id)
}

/// `save#EditBuffer`: Save: write a guided editor's staged files through the save pipeline, in kind order, each at
/// the hash its edits started from. A file changed on disk since stops the save there and is answered as the conflict
/// (the disk's text and the diff to the staged one); `overwrite` (`[{rootId, path}]`, "Save mine over it") saves
/// those over the disk's text. Then the data files marked to load are loaded, and the buffer is dropped. What was
/// written before a conflict or a failure stays written and leaves the buffer.
fn save_edit_buffer(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let id = buffer_id(parameters)?;
    let key = (user()?, id.clone());
    let files: Vec<BufferFile> = lock(store())
        .files(&key, Instant::now())
        .map(<[BufferFile]>::to_vec)
        .unwrap_or_default();
    let overwrite: Vec<(String, String)> = json_list(parameters, "overwrite")
        .iter()
        .filter_map(|file| Some((file["rootId"].as_str()?.to_string(), file["path"].as_str()?.to_string())))
        .collect();
    let saving = save_in_order(&files, |file| {
        let root = context.root(&file.root_id)?;
        let resolved = context.resolve(root, &file.path)?;
        let over = overwrite
            .iter()
            .any(|(root_id, path)| *root_id == file.root_id && *path == file.path);
        let base = match (over, disk(&context, &file.root_id, &file.path)?) {
            (true, Some((_, hash))) => hash,
            _ => file.base_hash.clone(),
        };
        let answer = crate::workspace::save_text(&context, root, &resolved, &file.text, &base, None)?;
        if answer.get("conflict") == Some(&Value::Bool(true)) {
            let text_of = |name: &str| {
                answer
                    .get(name)
                    .map(moqui_lib::object::to_plain_string)
                    .unwrap_or_default()
            };
            return Ok(Written::Conflict {
                disk_hash: text_of("diskHash"),
                disk_text: text_of("diskText"),
            });
        }
        Ok(Written::Saved(answer))
    });
    let mut out = Parameters::new();
    let mut saved = Vec::new();
    let mut restart = false;
    let mut problems = Vec::new();
    for (file, answer) in &saving.saved {
        let report = answer.get("reload").map(moqui_lib::json::to_json).unwrap_or(Json::Null);
        let summary: Vec<String> = report["files"]
            .as_array()
            .map(|files| {
                files
                    .iter()
                    .filter_map(|entry| entry["summary"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        restart |= answer.get("restart") == Some(&Value::Bool(true));
        if let Some(Json::Array(found)) = answer.get("problems").map(moqui_lib::json::to_json) {
            for mut problem in found {
                problem["file"] = json!(file.path);
                problems.push(problem);
            }
        }
        saved.push(
            json!({ "rootId": file.root_id, "path": file.path, "summary": summary.join("; "),
            "hash": answer.get("hash").map(moqui_lib::object::to_plain_string) }),
        );
    }
    {
        let mut kept = lock(store());
        for (file, _) in &saving.saved {
            kept.remove(&key, Some((&file.root_id, &file.path)));
        }
    }
    out.insert("saved".into(), json_value(&saved));
    out.insert("restart".into(), Value::Bool(restart));
    out.insert("problems".into(), json_value(&problems));
    if let Some((file, err)) = &saving.failed {
        let done: Vec<&str> = saving.saved.iter().map(|(file, _)| file.path.as_str()).collect();
        return Err(LibError::new(format!(
            "{}: {err}{}",
            file.path,
            if done.is_empty() {
                String::new()
            } else {
                format!(" (saved before it: {})", done.join(", "))
            }
        )));
    }
    if let Some((file, disk_hash, disk_text)) = &saving.conflict {
        out.insert(
            "conflict".into(),
            json_value(&json!({
                "rootId": file.root_id, "path": file.path, "diskHash": disk_hash,
                "diff": moqui_core::resource::textdiff::unified(disk_text, &file.text,
                    &format!("{} (disk)", file.path), &format!("{} (mine)", file.path)),
            })),
        );
        return Ok(out);
    }
    // the after-save steps: the seed rows of a Picklist or Status field load (the browser's afterSave)
    let mut loaded = Vec::new();
    for (file, _) in saving.saved.iter().filter(|(file, _)| file.load) {
        let result = context
            .root(&file.root_id)
            .and_then(|root| Ok((root, context.resolve(root, &file.path)?)))
            .and_then(|(root, resolved)| crate::entities::load_data_file(&context, root, &resolved));
        match result {
            Ok(records) => loaded.push(json!({ "path": file.path, "records": records })),
            // one file that does not load is said; the rest load (AGENTS.md failure tolerance)
            Err(err) => loaded.push(json!({ "path": file.path, "error": err.to_string() })),
        }
    }
    lock(store()).remove(&key, None);
    out.insert("loaded".into(), json_value(&loaded));
    Ok(out)
}

/// Each file of a canvas buffer as staged: its root, path, location and text.
pub(crate) fn canvas_file(context: &Context, root_id: &str, path: &str, body: &str) -> Result<BufferFile, LibError> {
    let root = context.root(root_id)?;
    let resolved = context.resolve(root, path)?;
    if body.len() as u64 > context.settings.max_edit_bytes {
        return Err(LibError::new(format!("{path} is over the edit limit")));
    }
    Ok(BufferFile {
        location: location(context, root_id, &resolved.relative)?,
        root_id: root_id.to_string(),
        path: resolved.relative,
        text: body.to_string(),
        base_hash: String::new(),
        load: false,
    })
}

/// The overlay of a `buffer:<id>` token: the current user's buffer of that id, a canvas's or a guided editor's (an
/// empty one renders the files).
pub(crate) fn buffer_overlay(factory: &ExecutionContextFactory, id: &str) -> Option<Overlay> {
    let user = moqui_core::user::current_user_id().ok().flatten()?;
    moqui_core::devtools::dev_aide_user(factory, &user).then_some(())?;
    let mut kept = lock(store());
    let mut overlay = Overlay::new();
    for file in kept.files(&(user, id.to_string()), Instant::now()).unwrap_or_default() {
        overlay.insert_location(factory, &file.location, &file.text);
    }
    Some(overlay)
}

#[cfg(test)]
#[path = "buffers_tests.rs"]
mod tests;
