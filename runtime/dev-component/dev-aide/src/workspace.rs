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

//! `org.moqui.devaide.WorkspaceServices`: the roots, their directories and files, and the save pipeline
//! (plans/design-dev-aide.md 4 and 5.7). A save writes the file (its line endings, byte order mark and mode kept; a
//! temporary file renamed over it), audits it, reloads what it defines, checks it, and answers the report with the
//! file's git status. A save over a file changed on disk since it was read is refused with the disk's text, which the
//! editor opens as Compare. An upstream file (a root no site component is in) is read only unless the user has the full
//! tier and "Allow editing upstream components" on (D17).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moqui_core::context::{Error, ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use sha2::{Digest, Sha256};

use crate::audit::{self, Write};
use crate::confine;
use crate::git::{self, Git};
use crate::roots::{self, Root, RootKind};
use crate::settings::Settings;
use crate::tier::{self, Tier};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(get_roots), get_roots)?;
    registry.service_function(moqui_core::service_location!(get_overview), get_overview)?;
    registry.service_function(moqui_core::service_location!(list_directory), list_directory)?;
    registry.service_function(moqui_core::service_location!(get_file), get_file)?;
    registry.service_function(moqui_core::service_location!(save_file), save_file)?;
    registry.service_function(moqui_core::service_location!(create_file), create_file)?;
    registry.service_function(moqui_core::service_location!(create_directory), create_directory)?;
    registry.service_function(moqui_core::service_location!(rename_path), rename_path)?;
    registry.service_function(moqui_core::service_location!(delete_path), delete_path)?;
    registry.service_function(moqui_core::service_location!(search_files), search_files)?;
    registry.service_function(moqui_core::service_location!(find_files), find_files)?;
    registry.service_function(moqui_core::service_location!(check_file), check_file)?;
    registry.service_function(moqui_core::service_location!(set_upstream_edits), set_upstream_edits)?;
    registry.service_function(moqui_core::service_location!(set_setting), set_setting)?;
    registry.service_function(moqui_core::service_location!(get_credentials), get_credentials)?;
    registry.service_function(moqui_core::service_location!(set_presence), set_presence)?;
    registry.service_function(moqui_core::service_location!(store_credential), store_credential)?;
    registry.service_function(moqui_core::service_location!(delete_credential), delete_credential)?;
    registry.service_function(moqui_core::service_location!(store_root), store_root)?;
    registry.service_function(moqui_core::service_location!(delete_root), delete_root)?;
    registry.service_function(
        moqui_core::service_location!(create_site_component),
        create_site_component,
    )
}

/// The preference that turns the upstream lock off for a full-tier user.
pub const UPSTREAM_EDITS: &str = "DEV_AIDE_UPSTREAM_EDITS";

/// What a service needs: the factory, the entity facade, the settings, the roots and the user's tier.
pub(crate) struct Context {
    pub factory: Arc<ExecutionContextFactory>,
    pub facade: EntityFacade,
    pub settings: Settings,
    pub roots: Vec<Root>,
    pub tier: Tier,
}

impl Context {
    /// The context of a call that needs `least`.
    pub(crate) fn of(ec: &ExecutionContext, least: Tier) -> Result<Self, LibError> {
        let tier = tier::require(least)?;
        let factory = Arc::clone(ec.factory());
        crate::notify::ensure(&factory);
        let facade = factory.entity().ok_or_else(|| LibError::new("no entity facade"))?;
        let settings = Settings::of(&factory, Some(&facade));
        let roots = roots::roots(&factory, &settings, Some(&facade));
        Ok(Self {
            factory,
            facade,
            settings,
            roots,
            tier,
        })
    }

    pub(crate) fn root(&self, id: &str) -> Result<&Root, LibError> {
        roots::find(&self.roots, id).ok_or_else(|| LibError::new(format!("no root {id}")))
    }

    pub(crate) fn resolve(&self, root: &Root, relative: &str) -> Result<confine::Resolved, LibError> {
        confine::resolve(&root.path, self.factory.runtime_dir(), relative)
    }

    pub(crate) fn git(&self) -> Git {
        Git {
            command: self.settings.git_command.clone(),
            read_timeout: self.settings.git_read_timeout,
            network_timeout: self.settings.git_network_timeout,
            write_timeout: self.settings.git_write_timeout,
        }
    }

    /// Whether this full-tier user turned the upstream lock off (the conf's allow-upstream-edits is the default).
    pub(crate) fn upstream_edits(&self) -> bool {
        if self.tier != Tier::Full {
            return false;
        }
        match moqui_core::user::get_preference(UPSTREAM_EDITS).ok().flatten() {
            Some(value) => value == "true",
            None => self.settings.allow_upstream_edits,
        }
    }

    /// Why the current user may not write `relative` in `root`, or `Ok` when they may.
    pub(crate) fn may_write(&self, root: &Root, relative: &str) -> Result<(), LibError> {
        if self.tier == Tier::View {
            return Err(LibError::new("Writing files needs the extend tier of Dev AIDE"));
        }
        if root.read_only {
            return Err(LibError::new(format!(
                "{} is read only in Dev AIDE Settings",
                root.label
            )));
        }
        if root.is_site_path(relative) || self.upstream_edits() {
            return Ok(());
        }
        let what = root
            .component_of(relative)
            .map_or_else(|| root.label.clone(), |component| component.name.clone());
        let site = self
            .settings
            .default_site()
            .map_or_else(|| "a site component (Home: Create one)".to_string(), str::to_string);
        Err(LibError::new(format!("{what} is upstream: extend it in {site}")))
    }
}

pub(crate) fn text(parameters: &Parameters, key: &str) -> String {
    parameters
        .get(key)
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

pub(crate) fn flag(parameters: &Parameters, key: &str) -> bool {
    matches!(text(parameters, key).as_str(), "true" | "Y" | "y" | "1")
}

/// A serde value as a service value.
pub(crate) fn json_value<T: serde::Serialize>(value: &T) -> Value {
    moqui_lib::json::from_json(&serde_json::to_value(value).unwrap_or(serde_json::Value::Null))
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// A file's bytes as an editor's text: the byte order mark taken off, `None` for bytes that are not UTF-8 text.
fn decode(bytes: &[u8]) -> Option<(String, bool)> {
    if confine::is_binary(bytes) {
        return None;
    }
    let (bom, body) = match bytes.strip_prefix(BOM) {
        Some(body) => (true, body),
        None => (false, bytes),
    };
    String::from_utf8(body.to_vec()).ok().map(|text| (text, bom))
}

/// CRLF when most of the text's line breaks are.
fn crlf(text: &str) -> bool {
    let breaks = text.matches('\n').count();
    breaks > 0 && text.matches("\r\n").count() * 2 > breaks
}

/// The git status letter of each path of `root` under `dir` (relative to the root), with each directory's rolled up.
fn statuses(context: &Context, root: &Root, dir: &str) -> Vec<(String, char)> {
    if root.kind != RootKind::Git {
        return Vec::new();
    }
    let mut args = vec!["status", "--porcelain=v2", "-z", "--untracked-files=all", "--ignored"];
    if !dir.is_empty() {
        args.extend(["--", dir]);
    }
    match context.git().read(&root.path, &args) {
        Ok(out) => git::parse_status(&out)
            .entries
            .into_iter()
            .map(|entry| (entry.path.trim_end_matches('/').to_string(), entry.letter()))
            .collect(),
        Err(err) => {
            tracing::warn!("git status in {}: {err}", root.path.display());
            Vec::new()
        }
    }
}

/// `get#Roots`: the roots, whether git runs, the user's tier and the site components.
fn get_roots(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    _parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let mut out = Parameters::new();
    out.insert("roots".into(), json_value(&context.roots));
    out.insert("gitAvailable".into(), Value::Bool(context.git().available()));
    out.insert("tier".into(), Value::string(context.tier.name()));
    out.insert("siteComponents".into(), json_value(&context.settings.site_components));
    out.insert("upstreamEdits".into(), Value::Bool(context.upstream_edits()));
    out.insert("shared".into(), Value::Bool(context.settings.shared));
    Ok(out)
}

/// `get#Overview`: what Home shows: each root with its branch and changes (the site components' apart), the reload log
/// (the last reports, newest first, a row per file), whether a reload since the boot asked for a restart, and the site
/// components.
fn get_overview(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    _parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let git = context.git();
    let git_available = git.available();
    let mut roots = Vec::new();
    let mut unpublished = 0_usize;
    for root in &context.roots {
        let mut row = serde_json::json!({
            "rootId": root.id, "label": root.label, "kind": root.kind, "upstream": root.upstream,
            "path": root.path.display().to_string(),
            "components": root.components.iter().map(|component| component.name.as_str()).collect::<Vec<_>>().join(", "),
        });
        if root.kind == RootKind::Git && git_available {
            if let Ok(out) = git.read(
                &root.path,
                &["status", "--porcelain=v2", "-z", "--branch", "--untracked-files=all"],
            ) {
                let status = git::parse_status(&out);
                let site = status
                    .entries
                    .iter()
                    .filter(|entry| root.is_site_path(&entry.path))
                    .count();
                unpublished += site;
                row["branch"] = serde_json::json!(status.branch.unwrap_or_else(|| "(detached)".into()));
                row["ahead"] = serde_json::json!(status.ahead);
                row["behind"] = serde_json::json!(status.behind);
                row["changes"] = serde_json::json!(status.entries.len());
                row["siteChanges"] = serde_json::json!(site);
            }
        }
        roots.push(row);
    }
    let reports = moqui_core::devtools::recent_reports(&context.factory);
    let restart = reports.iter().any(|report| report.restart);
    let rebuild = reports.iter().any(|report| report.rebuild);
    let reloads: Vec<serde_json::Value> = reports
        .iter()
        .take(20)
        .flat_map(|report| {
            report.files.iter().map(move |file| {
                serde_json::json!({
                    "at": report.at, "path": file.path, "component": file.component, "summary": file.summary,
                    "problems": file.problems.len(),
                    "problem": file.problems.first().map(|problem| problem.message.clone()),
                })
            })
        })
        .collect();
    let mut out = Parameters::new();
    out.insert("roots".into(), json_value(&roots));
    out.insert("reloads".into(), json_value(&reloads));
    out.insert("restartNeeded".into(), Value::Bool(restart));
    out.insert("rebuildNeeded".into(), Value::Bool(rebuild));
    out.insert(
        "unpublished".into(),
        Value::I64(i64::try_from(unpublished).unwrap_or(i64::MAX)),
    );
    out.insert("siteComponents".into(), json_value(&context.settings.site_components));
    out.insert("gitAvailable".into(), Value::Bool(git_available));
    out.insert("tier".into(), Value::string(context.tier.name()));
    Ok(out)
}

/// `list#Directory`: the entries of a directory of a root, folders first, each with its git status (a folder's rolled
/// up from what is in it) and whether git ignores it; `.git` is never listed.
fn list_directory(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = context.root(&text(parameters, "rootId"))?;
    let resolved = context.resolve(root, &text(parameters, "path"))?;
    let dir = resolved.relative.clone();
    let statuses = statuses(&context, root, &dir);
    let mut entries = Vec::new();
    let read = std::fs::read_dir(&resolved.path).map_err(|err| LibError::new(format!("{dir}: {err}")))?;
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if confine::HIDDEN.contains(&name.as_str()) {
            continue;
        }
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        let relative = if dir.is_empty() {
            name.clone()
        } else {
            format!("{dir}/{name}")
        };
        let prefix = format!("{relative}/");
        let mine: Vec<char> = statuses
            .iter()
            .filter(|(path, _)| *path == relative || (is_dir && path.starts_with(&prefix)))
            .map(|(_, letter)| *letter)
            .collect();
        let ignored = mine.contains(&'!') && mine.iter().all(|letter| *letter == '!');
        let status = mine
            .iter()
            .filter(|letter| **letter != '!')
            .copied()
            .min_by_key(|letter| "UMARD?".find(*letter).unwrap_or(9))
            .map(String::from)
            .unwrap_or_default();
        let modified = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(0));
        entries.push(serde_json::json!({
            "name": name, "dir": is_dir, "status": status, "ignored": ignored, "modified": modified,
        }));
    }
    entries.sort_by(|left, right| {
        (
            !left["dir"].as_bool().unwrap_or(false),
            left["name"].as_str().unwrap_or("").to_lowercase(),
        )
            .cmp(&(
                !right["dir"].as_bool().unwrap_or(false),
                right["name"].as_str().unwrap_or("").to_lowercase(),
            ))
    });
    let mut out = Parameters::new();
    out.insert("entries".into(), json_value(&entries));
    Ok(out)
}

/// `get#File`: a file's text with its hash, line ending, byte order mark, git status, and whether (and why) it is read
/// only; a binary file or one over the size limit has no text.
fn get_file(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = context.root(&text(parameters, "rootId"))?;
    let resolved = context.resolve(root, &text(parameters, "path"))?;
    let mut out = Parameters::new();
    if !resolved.path.exists() && text(parameters, "missingOk") == "true" {
        // a file a view makes on its first save (the site's extension file, a seed data file)
        out.insert("path".into(), Value::string(&resolved.relative));
        out.insert("missing".into(), Value::Bool(true));
        out.insert("binary".into(), Value::Bool(false));
        if let Err(err) = context.may_write(root, &resolved.relative) {
            out.insert("readOnly".into(), Value::Bool(true));
            out.insert("readOnlyReason".into(), Value::string(err.to_string()));
        }
        return Ok(out);
    }
    let bytes = std::fs::read(&resolved.path).map_err(|err| LibError::new(format!("{}: {err}", resolved.relative)))?;
    out.insert("path".into(), Value::string(&resolved.relative));
    out.insert("hash".into(), Value::string(sha256(&bytes)));
    out.insert(
        "size".into(),
        Value::I64(i64::try_from(bytes.len()).unwrap_or(i64::MAX)),
    );
    let mut read_only = context
        .may_write(root, &resolved.relative)
        .err()
        .map(|err| err.to_string());
    let too_big = bytes.len() as u64 > context.settings.max_edit_bytes;
    match decode(&bytes) {
        Some((text, bom)) if !too_big => {
            out.insert(
                "lineEnding".into(),
                Value::string(if crlf(&text) { "CRLF" } else { "LF" }),
            );
            out.insert("bom".into(), Value::Bool(bom));
            out.insert("text".into(), Value::string(text));
            out.insert("binary".into(), Value::Bool(false));
        }
        Some(_) => {
            read_only = Some(format!(
                "{} is over {} bytes, so it opens read only",
                resolved.relative, context.settings.max_edit_bytes
            ));
            out.insert("binary".into(), Value::Bool(false));
        }
        None => {
            read_only = Some("a binary file".into());
            out.insert("binary".into(), Value::Bool(true));
        }
    }
    let status = statuses(&context, root, &resolved.relative)
        .into_iter()
        .find(|(path, _)| *path == resolved.relative)
        .map(|(_, letter)| letter.to_string())
        .unwrap_or_default();
    out.insert("status".into(), Value::string(status));
    out.insert("readOnly".into(), Value::Bool(read_only.is_some()));
    out.insert("readOnlyReason".into(), read_only.map_or(Value::Null, Value::string));
    out.insert(
        "component".into(),
        root.component_of(&resolved.relative)
            .map_or(Value::Null, |component| Value::string(&component.name)),
    );
    Ok(out)
}

/// Write `bytes` over `path` through a temporary file beside it, keeping its permissions.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), LibError> {
    let dir = path.parent().ok_or_else(|| LibError::new("a path with no directory"))?;
    std::fs::create_dir_all(dir).map_err(|err| LibError::new(format!("{}: {err}", dir.display())))?;
    let name = path
        .file_name()
        .map_or_else(|| "file".into(), |name| name.to_string_lossy().into_owned());
    let temp = dir.join(format!(".{name}.moqui-save-{}", std::process::id()));
    std::fs::write(&temp, bytes).map_err(|err| LibError::new(format!("{}: {err}", temp.display())))?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&temp, meta.permissions());
    }
    std::fs::rename(&temp, path).map_err(|err| {
        let _ = std::fs::remove_file(&temp);
        LibError::new(format!("{}: {err}", path.display()))
    })
}

/// The reload and check of a written file, and its git status, into `out`.
fn after_write(context: &Context, root: &Root, resolved: &confine::Resolved, text: Option<&str>, out: &mut Parameters) {
    let report = moqui_core::devtools::reload_paths(&context.factory, std::slice::from_ref(&resolved.path));
    let mut problems: Vec<moqui_core::devtools::Problem> = report.problems().cloned().collect();
    if let Some(text) = text {
        if resolved.relative.ends_with(".xml") {
            problems.extend(moqui_core::devtools::check::check_text(
                &context.factory,
                &resolved.path,
                text,
            ));
        }
    }
    if !root.is_site_path(&resolved.relative) {
        // an edit of an upstream file while the lock is off is a fork; the check says so (D17)
        problems.push(moqui_core::devtools::Problem {
            path: resolved.path.display().to_string(),
            line: 0,
            column: 0,
            severity: moqui_core::devtools::Severity::Warning,
            rule: "upstream.edit".into(),
            message: format!(
                "{} is upstream: an edit here is a fork every upgrade has to merge; an extension in {} is not",
                root.component_of(&resolved.relative)
                    .map_or(root.label.as_str(), |component| component.name.as_str()),
                context.settings.default_site().unwrap_or("a site component")
            ),
            skill: Some(format!("{}#Extending", moqui_core::devtools::check::SKILL_FILE)),
        });
    }
    let status = statuses(context, root, &resolved.relative)
        .into_iter()
        .find(|(path, _)| *path == resolved.relative)
        .map(|(_, letter)| letter.to_string())
        .unwrap_or_default();
    out.insert("status".into(), Value::string(status));
    out.insert("restart".into(), Value::Bool(report.restart));
    out.insert("rebuild".into(), Value::Bool(report.rebuild));
    out.insert("reload".into(), json_value(&report));
    out.insert("problems".into(), json_value(&problems));
}

/// `save#File`: write a file the editor read (`baseHash` its hash then), or refuse with the disk's text when it changed
/// since; the report of the reload, the checks and the git status.
fn save_file(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = context.root(&text(parameters, "rootId"))?;
    let resolved = context.resolve(root, &text(parameters, "path"))?;
    save_text(
        &context,
        root,
        &resolved,
        &text(parameters, "text"),
        &text(parameters, "baseHash"),
        None,
    )
}

/// The save pipeline (plans/design-dev-aide.md 5.7), for an editor's Save and an accepted change set alike: the user
/// may write the file; a file there is still at `base_hash`, else the answer is the conflict with the disk's text; its line endings and byte order mark are kept; a temporary file is renamed over it;
/// the write is audited (with `detail`, such as the agent run and approval); and what it defines is reloaded and
/// checked, the answer carrying the report, the problems and the git status.
///
/// # Errors
///
/// Returns an error when the user may not write the file, the text is too long, or the file cannot be written.
pub(crate) fn save_text(
    context: &Context,
    root: &Root,
    resolved: &confine::Resolved,
    content: &str,
    base_hash: &str,
    detail: Option<serde_json::Value>,
) -> Result<Parameters, LibError> {
    context.may_write(root, &resolved.relative)?;
    if content.len() as u64 > context.settings.max_edit_bytes {
        return Err(LibError::new(format!(
            "the text is over {} bytes",
            context.settings.max_edit_bytes
        )));
    }
    let mut out = Parameters::new();
    let existing = std::fs::read(&resolved.path).ok();
    let (crlf_file, bom) = match &existing {
        Some(bytes) => {
            let disk_hash = sha256(bytes);
            if disk_hash != base_hash {
                out.insert("conflict".into(), Value::Bool(true));
                out.insert("diskHash".into(), Value::string(disk_hash));
                out.insert(
                    "diskText".into(),
                    decode(bytes).map_or(Value::Null, |(text, _)| Value::string(text)),
                );
                return Ok(out);
            }
            let (disk_text, bom) = decode(bytes).ok_or_else(|| LibError::new("a binary file is not saved as text"))?;
            (crlf(&disk_text), bom)
        }
        None => (false, false),
    };
    let normalized = content.replace("\r\n", "\n");
    let body = if crlf_file {
        normalized.replace('\n', "\r\n")
    } else {
        normalized
    };
    let mut bytes = Vec::with_capacity(body.len() + 3);
    if bom {
        bytes.extend_from_slice(BOM);
    }
    bytes.extend_from_slice(body.as_bytes());
    if let Some(parent) = resolved.path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| LibError::new(format!("{}: {err}", resolved.relative)))?;
    }
    write_atomic(&resolved.path, &bytes)?;
    let hash = sha256(&bytes);
    audit::record(
        &context.facade,
        &Write {
            action: if existing.is_some() { "DAA_SAVE" } else { "DAA_CREATE" },
            root: &root.id,
            path: &resolved.relative,
            hash_before: existing.as_ref().map(|_| base_hash),
            hash_after: Some(&hash),
            detail,
        },
    );
    out.insert("conflict".into(), Value::Bool(false));
    out.insert("hash".into(), Value::string(&hash));
    after_write(context, root, resolved, Some(content), &mut out);
    if context.settings.shared {
        let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
        let username = moqui_core::user::current_username().ok().flatten().unwrap_or_default();
        crate::notify::announce_save(&context.facade, &user, &username, &root.id, &resolved.relative);
    }
    Ok(out)
}

/// `set#Presence`: the files of a root this user has open (none when the page closes), on a shared dev server told to
/// the other users (D19); the answer is the root's other editors.
fn set_presence(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let _authz = ec.disable_authz();
    let root = context.root(&text(parameters, "rootId"))?.id.clone();
    let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    let mut out = Parameters::new();
    if !context.settings.shared || user.is_empty() {
        out.insert("editors".into(), Value::List(Vec::new().into()));
        return Ok(out);
    }
    let paths: Vec<String> = match parameters.get("paths") {
        Some(Value::List(items)) => items.iter().map(moqui_lib::object::to_plain_string).collect(),
        _ => text(parameters, "paths")
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect(),
    };
    let username = moqui_core::user::current_username().ok().flatten().unwrap_or_default();
    crate::notify::set_presence(&context.facade, &user, &username, &root, paths);
    let editors = crate::notify::editors(&root, &user);
    out.insert("editors".into(), json_value(&editors["editors"]));
    Ok(out)
}

/// `create#File`: a new file with `text` (empty by default); refused when one is there.
fn create_file(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = context.root(&text(parameters, "rootId"))?;
    let resolved = context.resolve(root, &text(parameters, "path"))?;
    context.may_write(root, &resolved.relative)?;
    if resolved.path.exists() {
        return Err(LibError::new(format!("{} is there already", resolved.relative)));
    }
    let content = text(parameters, "text");
    write_atomic(&resolved.path, content.as_bytes())?;
    let hash = sha256(content.as_bytes());
    audit::record(
        &context.facade,
        &Write {
            action: "DAA_CREATE",
            root: &root.id,
            path: &resolved.relative,
            hash_after: Some(&hash),
            ..Write::default()
        },
    );
    let mut out = Parameters::new();
    out.insert("hash".into(), Value::string(&hash));
    out.insert("path".into(), Value::string(&resolved.relative));
    after_write(&context, root, &resolved, Some(&content), &mut out);
    Ok(out)
}

/// `create#Directory`.
fn create_directory(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = context.root(&text(parameters, "rootId"))?;
    let resolved = context.resolve(root, &text(parameters, "path"))?;
    context.may_write(root, &resolved.relative)?;
    std::fs::create_dir_all(&resolved.path).map_err(|err| LibError::new(format!("{}: {err}", resolved.relative)))?;
    audit::record(
        &context.facade,
        &Write {
            action: "DAA_CREATE",
            root: &root.id,
            path: &resolved.relative,
            ..Write::default()
        },
    );
    let mut out = Parameters::new();
    out.insert("path".into(), Value::string(&resolved.relative));
    Ok(out)
}

/// `rename#Path`: rename or move a file or directory within its root; both places must be writable. With `git` in a
/// repository the move is `git mv`, so the history follows.
fn rename_path(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = context.root(&text(parameters, "rootId"))?;
    let from = context.resolve(root, &text(parameters, "path"))?;
    let to = context.resolve(root, &text(parameters, "to"))?;
    context.may_write(root, &from.relative)?;
    context.may_write(root, &to.relative)?;
    if !from.path.exists() {
        return Err(LibError::new(format!("{} is not there", from.relative)));
    }
    if to.path.exists() {
        return Err(LibError::new(format!("{} is there already", to.relative)));
    }
    if let Some(parent) = to.path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| LibError::new(err.to_string()))?;
    }
    let tracked = root.kind == RootKind::Git
        && context
            .git()
            .run(
                &root.path,
                git::Mode::Read,
                &["ls-files", "--error-unmatch", "--", &from.relative],
                &[],
                None,
            )
            .is_ok_and(|output| output.success());
    if tracked {
        context
            .git()
            .run(
                &root.path,
                git::Mode::Write,
                &["mv", "--", &from.relative, &to.relative],
                &[],
                None,
            )?
            .checked("mv")?;
    } else {
        std::fs::rename(&from.path, &to.path).map_err(|err| LibError::new(err.to_string()))?;
    }
    audit::record(
        &context.facade,
        &Write {
            action: "DAA_MOVE",
            root: &root.id,
            path: &from.relative,
            detail: Some(serde_json::json!({ "to": to.relative })),
            ..Write::default()
        },
    );
    let report = moqui_core::devtools::reload_paths(&context.factory, &[from.path.clone(), to.path.clone()]);
    let mut out = Parameters::new();
    out.insert("path".into(), Value::string(&to.relative));
    out.insert("reload".into(), json_value(&report));
    Ok(out)
}

/// `delete#Path`: delete a file, or a directory with what is in it.
fn delete_path(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = context.root(&text(parameters, "rootId"))?;
    let resolved = context.resolve(root, &text(parameters, "path"))?;
    if resolved.relative.is_empty() {
        return Err(LibError::new("a root is not deleted from here"));
    }
    context.may_write(root, &resolved.relative)?;
    let before = std::fs::read(&resolved.path).ok().map(|bytes| sha256(&bytes));
    if resolved.path.is_dir() {
        std::fs::remove_dir_all(&resolved.path)
    } else {
        std::fs::remove_file(&resolved.path)
    }
    .map_err(|err| LibError::new(format!("{}: {err}", resolved.relative)))?;
    audit::record(
        &context.facade,
        &Write {
            action: "DAA_DELETE",
            root: &root.id,
            path: &resolved.relative,
            hash_before: before.as_deref(),
            ..Write::default()
        },
    );
    let report = moqui_core::devtools::reload_paths(&context.factory, std::slice::from_ref(&resolved.path));
    let mut out = Parameters::new();
    out.insert("reload".into(), json_value(&report));
    Ok(out)
}

/// The files of a root a search or Go to file reads: what git tracks and the untracked files it does not ignore, or a
/// walk of a plain root that leaves out hidden directories and build output.
fn root_files(context: &Context, root: &Root, limit: usize) -> Vec<String> {
    if root.kind == RootKind::Git {
        if let Ok(out) = context.git().read(
            &root.path,
            &["ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        ) {
            return out
                .split('\0')
                .filter(|path| !path.is_empty())
                .take(limit)
                .map(str::to_string)
                .collect();
        }
    }
    let mut files = Vec::new();
    let mut stack = vec![(root.path.clone(), String::new())];
    while let Some((dir, relative)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            let path = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                stack.push((entry.path(), path));
            } else {
                files.push(path);
                if files.len() >= limit {
                    return files;
                }
            }
        }
    }
    files.sort();
    files
}

/// `find#Files`: every file of a root, for Go to file (the client filters).
fn find_files(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = context.root(&text(parameters, "rootId"))?;
    let files = root_files(&context, root, 50_000);
    let mut out = Parameters::new();
    out.insert("files".into(), json_value(&files));
    Ok(out)
}

/// `search#Files`: lines matching `query` (text, or a regular expression with `regex`), at most 500, with `git grep`
/// in a repository (its .gitignore respected) and a walk of the files in a plain root.
fn search_files(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = context.root(&text(parameters, "rootId"))?;
    let query = text(parameters, "query");
    if query.is_empty() {
        return Err(LibError::new("nothing to search for"));
    }
    let regex = flag(parameters, "regex");
    let case = flag(parameters, "caseSensitive");
    let filter = text(parameters, "pathFilter");
    const LIMIT: usize = 500;
    let mut results = Vec::new();
    if root.kind == RootKind::Git {
        let mut args = vec!["grep", "-n", "-I", "--no-color", "-z", "--untracked"];
        args.push(if regex { "-E" } else { "-F" });
        if !case {
            args.push("-i");
        }
        args.extend(["-e", &query, "--"]);
        if !filter.is_empty() {
            args.push(&filter);
        }
        let output = context.git().run(&root.path, git::Mode::Read, &args, &[], None)?;
        // exit 1 is "no match"
        if output.code != Some(0) && output.code != Some(1) {
            return Err(LibError::new(format!("git grep: {}", output.stderr.trim())));
        }
        for line in output.stdout.lines().take(LIMIT) {
            let mut parts = line.splitn(3, '\0');
            if let (Some(path), Some(number), Some(text)) = (parts.next(), parts.next(), parts.next()) {
                results.push(
                    serde_json::json!({ "path": path, "line": number.parse::<u64>().unwrap_or(0), "text": text }),
                );
            }
        }
    } else {
        let pattern = if regex { query.clone() } else { regex::escape(&query) };
        let matcher = regex::RegexBuilder::new(&pattern)
            .case_insensitive(!case)
            .build()
            .map_err(|err| LibError::new(format!("the regular expression: {err}")))?;
        'files: for path in root_files(&context, root, 20_000) {
            if !filter.is_empty() && !path.contains(filter.trim_matches('*')) {
                continue;
            }
            let Ok(bytes) = std::fs::read(root.path.join(&path)) else {
                continue;
            };
            let Some((body, _)) = decode(&bytes) else {
                continue;
            };
            for (index, line) in body.lines().enumerate() {
                if matcher.is_match(line) {
                    results.push(serde_json::json!({ "path": path, "line": index + 1, "text": line }));
                    if results.len() >= LIMIT {
                        break 'files;
                    }
                }
            }
        }
    }
    let mut out = Parameters::new();
    out.insert("truncated".into(), Value::Bool(results.len() >= LIMIT));
    out.insert("results".into(), json_value(&results));
    Ok(out)
}

/// `check#File`: the problems of a file's text (an editor's buffer), as the save would find them.
fn check_file(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = context.root(&text(parameters, "rootId"))?;
    let resolved = context.resolve(root, &text(parameters, "path"))?;
    let content = text(parameters, "text");
    let problems = if resolved.relative.ends_with(".xml") {
        moqui_core::devtools::check::check_text(&context.factory, &resolved.path, &content)
    } else {
        Vec::new()
    };
    let mut out = Parameters::new();
    out.insert("problems".into(), json_value(&problems));
    Ok(out)
}

/// `set#UpstreamEdits`: a full-tier user's "Allow editing upstream components" (D17).
fn set_upstream_edits(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let _context = Context::of(ec, Tier::Full)?;
    let on = flag(parameters, "enabled");
    moqui_core::user::set_preference(UPSTREAM_EDITS, Some(if on { "true" } else { "false" }))?;
    if on {
        tracing::warn!(
            "Dev AIDE: user {} turned on editing upstream components",
            moqui_core::user::current_username().ok().flatten().unwrap_or_default()
        );
    }
    let mut out = Parameters::new();
    out.insert("upstreamEdits".into(), Value::Bool(on));
    Ok(out)
}

/// `set#Setting`: an instance-wide setting of Settings (siteComponents, networkGit), for the full tier; network git in
/// shared mode is for admins.
fn set_setting(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let _context = Context::of(ec, Tier::Full)?;
    let _authz = ec.disable_authz();
    let name = text(parameters, "settingName");
    if !matches!(name.as_str(), "siteComponents" | "networkGit") {
        return Err(LibError::new(format!("{name} is not a Dev AIDE setting")));
    }
    if name == "networkGit" && !moqui_core::user::is_in_group("ADMIN").unwrap_or(false) {
        return Err(LibError::new(
            "Network git on a shared dev server is enabled by an admin",
        ));
    }
    let value = text(parameters, "settingValue");
    let facade = ec.factory().entity().ok_or_else(|| LibError::new("no entity facade"))?;
    let mut row = facade.make_value(crate::settings::SETTING)?;
    row.set("settingName", Value::string(&name));
    row.set("settingValue", Value::string(value.trim()));
    row.create_or_update(&facade)?;
    tracing::info!(
        "Dev AIDE setting {name} set to {} by {}",
        value.trim(),
        moqui_core::user::current_username().ok().flatten().unwrap_or_default()
    );
    Ok(Parameters::new())
}

const CREDENTIAL: &str = "moqui.dev.DevAideCredential";

fn require_admin(what: &str) -> Result<(), LibError> {
    if moqui_core::user::is_in_group("ADMIN").unwrap_or(false) {
        Ok(())
    } else {
        Err(LibError::new(format!("{what} is set by an admin")))
    }
}

/// `get#Credentials`: the shared server's stored tokens (D19), without the tokens: each one's root, user and user name.
fn get_credentials(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    _parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    require_admin("A git token")?;
    let _authz = ec.disable_authz();
    let rows = context.facade.find(CREDENTIAL)?.order_by("rootId", false).list()?;
    let field = |row: &moqui_core::entity::EntityValue, name: &str| {
        row.get(name)
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default()
    };
    let credentials: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let root = field(row, "rootId");
            serde_json::json!({
                "credentialId": field(row, "credentialId"),
                "rootId": root,
                "rootLabel": roots::find(&context.roots, &root).map_or_else(|| root.clone(), |root| root.label.clone()),
                "userId": field(row, "userId"),
                "username": field(row, "username"),
            })
        })
        .collect();
    let mut out = Parameters::new();
    out.insert("credentials".into(), json_value(&credentials));
    Ok(out)
}

/// `store#Credential`: a root's deploy token (no `userId`) or a user's HTTPS token for it, kept encrypted; one per root
/// and user, replaced when stored again. An admin's.
fn store_credential(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    require_admin("A git token")?;
    let _authz = ec.disable_authz();
    let root = context.root(&text(parameters, "rootId"))?.id.clone();
    let user_id = text(parameters, "userId");
    let token = text(parameters, "token");
    if token.trim().is_empty() {
        return Err(LibError::new("a token is required"));
    }
    let mut find = context.facade.find(CREDENTIAL)?.eq("rootId", Value::string(&root));
    find = if user_id.is_empty() {
        find.eq("userId", Value::Null)
    } else {
        find.eq("userId", Value::string(&user_id))
    };
    for old in find.list()?.iter() {
        old.delete(&context.facade)?;
    }
    let mut row = context.facade.make_value(CREDENTIAL)?;
    row.set_sequenced_id_primary(&context.facade)?;
    row.set("rootId", Value::string(&root));
    if !user_id.is_empty() {
        row.set("userId", Value::string(&user_id));
    }
    row.set("username", Value::string(text(parameters, "username").trim()));
    row.set("token", Value::string(token.trim()));
    row.create(&context.facade)?;
    tracing::info!(
        "Dev AIDE git token for {root}{} stored by {}",
        if user_id.is_empty() {
            String::new()
        } else {
            format!(" and user {user_id}")
        },
        moqui_core::user::current_username().ok().flatten().unwrap_or_default()
    );
    Ok(Parameters::new())
}

/// `delete#Credential`: a stored token, by its id. An admin's.
fn delete_credential(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    require_admin("A git token")?;
    let _authz = ec.disable_authz();
    let found = context
        .facade
        .find(CREDENTIAL)?
        .eq("credentialId", Value::string(text(parameters, "credentialId")))
        .one()?;
    if let Some(row) = found {
        row.delete(&context.facade)?;
    }
    Ok(Parameters::new())
}

/// `store#Root`: a root of Settings by its location: a directory to add, or the label, read only flag and publish branch
/// of a discovered one.
fn store_root(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    let _authz = ec.disable_authz();
    let location = PathBuf::from(text(parameters, "location"));
    let location = location
        .canonicalize()
        .map_err(|err| LibError::new(format!("{}: {err}", location.display())))?;
    if !location.is_dir() {
        return Err(LibError::new(format!("{} is not a directory", location.display())));
    }
    let runtime = context.factory.runtime_dir().canonicalize().unwrap_or_default();
    for denied in confine::DENIED_RUNTIME {
        if confine::inside(&location, &runtime.join(denied)) {
            return Err(LibError::new(format!("runtime/{denied} is not a root")));
        }
    }
    let mut row = match roots::root_row(&context.facade, &location) {
        Some(row) => row,
        None => {
            let mut row = context.facade.make_value(roots::ROOT_ENTITY)?;
            row.set_sequenced_id_primary(&context.facade)?;
            row.set("location", Value::string(location.display().to_string()));
            row
        }
    };
    for field in ["label", "publishBranch"] {
        if parameters.contains_key(field) {
            row.set(field, Value::string(text(parameters, field).trim()));
        }
    }
    for field in ["readOnly", "publishBranchRequired"] {
        if parameters.contains_key(field) {
            row.set(field, Value::string(if flag(parameters, field) { "Y" } else { "N" }));
        }
    }
    row.create_or_update(&context.facade)?;
    let mut out = Parameters::new();
    out.insert("rootId".into(), row.get("rootId").cloned().unwrap_or(Value::Null));
    Ok(out)
}

/// `delete#Root`: a root row of Settings (a discovered root stays, with its defaults).
fn delete_root(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    let _authz = ec.disable_authz();
    let location = PathBuf::from(text(parameters, "location"));
    let location = location.canonicalize().unwrap_or(location);
    if let Some(row) = roots::root_row(&context.facade, &location) {
        row.delete(&context.facade)?;
    }
    Ok(Parameters::new())
}

/// `create#SiteComponent`: a component the site owns, from the dev-aide's Site template (plans/design-dev-aide.md 12), in
/// runtime/component/<name> with a git repository of its own; the site component when none is set. It loads after a
/// restart.
fn create_site_component(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    let _authz = ec.disable_authz();
    let name = text(parameters, "componentName");
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        && name.chars().next().is_some_and(|ch| ch.is_ascii_alphabetic());
    if !valid {
        return Err(LibError::new(
            "a component name starts with a letter and has letters, digits, - and _",
        ));
    }
    let depends: Vec<String> = text(parameters, "dependsOn")
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    let target = context.factory.runtime_dir().join("component").join(&name);
    if target.exists() {
        return Err(LibError::new(format!("runtime/component/{name} is there already")));
    }
    let template = context
        .factory
        .components()
        .iter()
        .find(|component| component.name == "dev-aide")
        .map(|component| component.location.join("template/site"))
        .ok_or_else(|| LibError::new("the dev-aide component is not loaded"))?;
    let class = pascal(&name);
    let depends_xml: String = depends
        .iter()
        .map(|depend| format!("\n    <depends-on name=\"{depend}\"/>"))
        .collect();
    copy_template(
        &template,
        &target,
        &|text: &str| {
            text.replace("@component@", &name)
                .replace("@Class@", &class)
                .replace("@depends@", &depends_xml)
        },
        &|file_name: &str| file_name.replace("Site", &class),
    )?;
    let git = context.git();
    if git.available() {
        if let Err(err) = git
            .run(&target, git::Mode::Write, &["init", "-q"], &[], None)
            .and_then(|output| output.checked("init"))
        {
            tracing::warn!("git init in {}: {err}", target.display());
        }
    }
    if context.settings.site_components.is_empty() {
        let mut row = context.facade.make_value(crate::settings::SETTING)?;
        row.set("settingName", Value::string("siteComponents"));
        row.set("settingValue", Value::string(&name));
        row.create_or_update(&context.facade)?;
    }
    audit::record(
        &context.facade,
        &Write {
            action: "DAA_CREATE",
            root: "component",
            path: &name,
            detail: Some(serde_json::json!({ "template": "site" })),
            ..Write::default()
        },
    );
    let mut out = Parameters::new();
    out.insert("location".into(), Value::string(target.display().to_string()));
    out.insert("restart".into(), Value::Bool(true));
    Ok(out)
}

/// `my-site` as `MySite`.
fn pascal(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect()
}

fn copy_template(
    from: &Path,
    to: &Path,
    content: &dyn Fn(&str) -> String,
    file_name: &dyn Fn(&str) -> String,
) -> Result<(), LibError> {
    std::fs::create_dir_all(to).map_err(|err| LibError::new(format!("{}: {err}", to.display())))?;
    let entries = std::fs::read_dir(from).map_err(|err| LibError::new(format!("{}: {err}", from.display())))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let target: PathBuf = to.join(file_name(&name));
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            copy_template(&entry.path(), &target, content, file_name)?;
        } else {
            let text = std::fs::read_to_string(entry.path()).map_err(|err| LibError::new(err.to_string()))?;
            std::fs::write(&target, content(&text))
                .map_err(|err| LibError::new(format!("{}: {err}", target.display())))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoding_and_line_endings() {
        assert_eq!(decode(b"\xEF\xBB\xBF<a/>"), Some(("<a/>".to_string(), true)));
        assert_eq!(decode(b"<a/>"), Some(("<a/>".to_string(), false)));
        assert_eq!(decode(b"\x89PNG\0"), None);
        assert_eq!(decode(&[0xff, 0xfe, 0x41]), None);
        assert!(crlf("a\r\nb\r\nc\n"));
        assert!(!crlf("a\nb\r\nc\n"));
        assert!(!crlf("one line"));
        assert_eq!(pascal("my-site"), "MySite");
        assert_eq!(pascal("acme_corp"), "AcmeCorp");
        assert_eq!(sha256(b"").len(), 64);
    }
}
