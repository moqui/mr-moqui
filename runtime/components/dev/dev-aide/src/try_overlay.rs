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

//! What a Try in sim runs over (plans/archive/design-dev-aide.md F14 for services, 9.4, 13.3): the unsaved texts of the
//! Workspace's buffers (`files`, each `{rootId, path, text}`), a canvas or guided editor's buffer (`buffer:<id>`), or a
//! change set of the user's own (`approval:<id>`), as the framework's service overlay
//! (`moqui_core::service::overlay`): the service files' definitions and the script files' texts at their locations,
//! held on the sim's thread only. Nothing of it is written, and it is held in the sim, never for a real run.

use std::sync::Arc;

use moqui_core::native::Parameters;
use moqui_core::service::overlay::{OverlayFile, ServiceOverlay};
use moqui_lib::error::Error as LibError;
use serde::Deserialize;
use serde_json::{json, Value as Json};

use crate::workspace::{text, Context};

/// The most files one Try runs over.
const MAX_FILES: usize = 20;

/// A file of a root with its text, as the overlay takes it.
fn root_file(context: &Context, root_id: &str, path: &str, body: &str) -> Result<OverlayFile, LibError> {
    let staged = crate::buffers::canvas_file(context, root_id, path, body)?;
    let root = context.root(root_id)?;
    let resolved = context.resolve(root, path)?;
    Ok(OverlayFile {
        location: staged.location,
        path: resolved.path,
        text: staged.text,
    })
}

/// The files of `buffer:<id>`: the current user's buffer of that id.
fn buffer_files(context: &Context, id: &str) -> Result<Vec<OverlayFile>, LibError> {
    let user = moqui_core::user::current_user_id()?.unwrap_or_default();
    let kept: Vec<crate::buffers::BufferFile> = moqui_lib::sync::lock(crate::buffers::store())
        .files(&(user, id.to_string()), std::time::Instant::now())
        .map(<[crate::buffers::BufferFile]>::to_vec)
        .ok_or_else(|| LibError::new(format!("no buffer {id} of yours")))?;
    kept.iter()
        .map(|file| root_file(context, &file.root_id, &file.path, &file.text))
        .collect()
}

/// The files of a change set of the current user's (`approvalId`), as proposed: each file's text with its change
/// applied to the file as it is now.
pub(crate) fn change_set_files(context: &Context, approval_id: &str) -> Result<Vec<OverlayFile>, LibError> {
    let view = crate::changeset::owned(context, approval_id)?;
    let set = crate::agent::propose::ChangeSet::deserialize(&view.args)
        .map_err(|err| LibError::new(format!("the change set does not read: {err}")))?;
    proposed_files(context, &set)
}

/// The files of a change set's proposal (its `files` as `propose_edit` takes them), its changes applied to the files
/// as they are now.
pub(crate) fn proposed_files(
    context: &Context,
    set: &crate::agent::propose::ChangeSet,
) -> Result<Vec<OverlayFile>, LibError> {
    let prepared = crate::agent::propose::prepare(context, set).map_err(|err| LibError::new(err.instruction))?;
    Ok(prepared
        .into_iter()
        .map(|file| {
            // a session's worktree file stands at its main tree's place, where the running server reads that file's
            // services and scripts, so its service group is the loaded one (21.8)
            let main = file
                .named
                .root
                .worktree_of
                .as_deref()
                .and_then(|main| context.root(main).ok());
            match main {
                Some(main) => OverlayFile {
                    location: crate::screens::location_of(main, &file.named.relative).unwrap_or(file.location),
                    path: main.path.join(&file.named.relative),
                    text: file.new_text,
                },
                None => OverlayFile {
                    location: file.location,
                    path: file.named.path.clone(),
                    text: file.new_text,
                },
            }
        })
        .collect())
}

/// The files a Try names: `files` (a JSON list of `{rootId, path, text}`) and `preview` (`buffer:<id>` or
/// `approval:<id>`); none when it names neither.
pub(crate) fn files_of(context: &Context, parameters: &Parameters) -> Result<Vec<OverlayFile>, LibError> {
    let mut files = Vec::new();
    for file in crate::buffers::json_list(parameters, "files").iter().take(MAX_FILES) {
        files.push(root_file(
            context,
            file["rootId"].as_str().unwrap_or_default(),
            file["path"].as_str().unwrap_or_default(),
            file["text"].as_str().unwrap_or_default(),
        )?);
    }
    let preview = text(parameters, "preview");
    if let Some(id) = preview.strip_prefix("buffer:") {
        files.extend(buffer_files(context, id)?);
    } else if let Some(id) = preview.strip_prefix("approval:") {
        files.extend(change_set_files(context, id)?);
    } else if !preview.is_empty() {
        return Err(LibError::new(format!(
            "preview {preview} is not buffer:<id> or approval:<id>"
        )));
    }
    files.truncate(MAX_FILES);
    Ok(files)
}

/// The overlay of `files`, none when they hold no service or script file.
///
/// # Errors
///
/// A service file that does not parse.
pub(crate) fn overlay_of(files: &[OverlayFile]) -> Result<Option<Arc<ServiceOverlay>>, LibError> {
    if files.is_empty() {
        return Ok(None);
    }
    let overlay = ServiceOverlay::build(files).map_err(|err| LibError::new(err.to_string()))?;
    Ok((!overlay.is_empty()).then(|| Arc::new(overlay)))
}

/// What a Try answers of its overlay: the services its files define, the script locations it holds, and the files'
/// registration failures.
pub(crate) fn overlay_json(overlay: &ServiceOverlay) -> Json {
    json!({ "services": overlay.defined(), "scripts": overlay.script_locations(), "failures": overlay.failures() })
}

/// Run `body` with `overlay` held on this thread when there is one.
pub(crate) fn over<R>(overlay: Option<Arc<ServiceOverlay>>, body: impl FnOnce() -> R) -> R {
    match overlay {
        Some(overlay) => moqui_core::service::overlay::with(overlay, body),
        None => body(),
    }
}
