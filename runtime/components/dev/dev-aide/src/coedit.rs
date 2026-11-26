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

//! Co-editing a file on a shared dev server (plans/design-dev-aide.md F15): `get#CoeditAccess`, which moqui-collab's
//! text rooms ask (`moqui_collab::text`) who may join the room of a root's file, as the user. Only in shared mode
//! (`dev-aide shared="true"`), only for a user who may write the file as Save would (the tier, the upstream lock, a
//! read-only root: [`Context::may_write`]), the path confined as a read's is, and only the path as the Workspace names
//! it (one room per file); a hosted root's files, a binary file and one over `max-edit-bytes` are not co-edited. The
//! answer has the file's text when the room is new, the most bytes of the document (`max-edit-bytes`) and the most
//! files a user co-edits at once (`dev-aide@coedit-max-rooms`) and the most bytes of every file's room on the server
//! together (`dev-aide@coedit-max-total-bytes`); its line endings are LF, as the editor's are. Save stays
//! the editor's own (`save#File`, by the hash of the file the user read).

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use crate::tier::Tier;
use crate::workspace::{decode, flag, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(get_coedit_access), get_coedit_access)
}

/// The most files a user co-edits at once when the conf gives no number.
const DEFAULT_MAX_ROOMS: i64 = 20;
/// The most bytes of every file's room together when the conf gives no number.
const DEFAULT_MAX_TOTAL_BYTES: i64 = 64 * 1024 * 1024;

/// `get#CoeditAccess`: whether `userId` may co-edit `path` of `rootId`, and why not; the bounds; with `withText` the
/// file's text. Asked by a room (a context with no user) or for the current user, never for another user.
fn get_coedit_access(
    _ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let user_id = text(parameters, "userId");
    if user_id.is_empty() {
        return Err(LibError::new("userId is required"));
    }
    let current = moqui_core::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty());
    if current.is_some_and(|current| current != user_id) {
        return Err(LibError::new("co-editing access is answered for the user only"));
    }
    let root_id = text(parameters, "rootId");
    let path = text(parameters, "path");
    let with_text = flag(parameters, "withText");
    moqui_core::user::as_user(&user_id, || {
        ExecutionContext::with_current(|ec| Ok(answer(ec, &root_id, &path, with_text)))
            .unwrap_or_else(|| Err(LibError::new("no execution context")))
    })
    .ok_or_else(|| LibError::new(format!("no user {user_id}")))?
}

/// The answer as the current user: `canEdit`, `reason` when not, the bounds, and the text when asked.
fn answer(ec: &ExecutionContext, root_id: &str, path: &str, with_text: bool) -> Parameters {
    let mut out = Parameters::new();
    let number = |name: &str, default: i64| {
        ec.factory()
            .conf_setting("dev-aide", name)
            .parse::<i64>()
            .ok()
            .filter(|max| *max > 0)
            .unwrap_or(default)
    };
    out.insert(
        "maxRooms".into(),
        Value::I64(number("coedit-max-rooms", DEFAULT_MAX_ROOMS)),
    );
    out.insert(
        "maxTotalBytes".into(),
        Value::I64(number("coedit-max-total-bytes", DEFAULT_MAX_TOTAL_BYTES)),
    );
    match readable(ec, root_id, path) {
        Ok((max_bytes, file_text)) => {
            out.insert("maxBytes".into(), Value::I64(max_bytes));
            out.insert("canEdit".into(), Value::Bool(true));
            if with_text {
                out.insert("text".into(), Value::string(file_text));
            }
        }
        Err(reason) => {
            out.insert("canEdit".into(), Value::Bool(false));
            out.insert("reason".into(), Value::string(reason.to_string()));
        }
    }
    out
}

/// The most bytes of the file's text and the text, when the current user may co-edit it; why not otherwise.
fn readable(ec: &ExecutionContext, root_id: &str, path: &str) -> Result<(i64, String), LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    if !context.settings.shared {
        return Err(LibError::new(
            "files are co-edited on a shared dev server only (dev-aide shared=\"true\")",
        ));
    }
    let _authz = ec.disable_authz();
    let root = context.root(root_id)?;
    if root.hosted_branch.is_some() {
        return Err(LibError::new(format!(
            "{} is a hosted repository: its files are not co-edited",
            root.label
        )));
    }
    let resolved = context.resolve(root, path)?;
    if resolved.relative != path {
        return Err(LibError::new(format!("{path} is {} in this root", resolved.relative)));
    }
    context.may_write(root, &resolved.relative)?;
    let bytes = std::fs::read(&resolved.path).map_err(|err| LibError::new(format!("{path}: {err}")))?;
    let max = context.settings.max_edit_bytes;
    if bytes.len() as u64 > max {
        return Err(LibError::new(format!("{path} is over {max} bytes")));
    }
    let (file_text, _) = decode(&bytes).ok_or_else(|| LibError::new(format!("{path} is a binary file")))?;
    // the editor's lines end in LF, as CodeMirror keeps them; Save writes the file's own endings back
    let file_text = file_text.replace("\r\n", "\n").replace('\r', "\n");
    Ok((i64::try_from(max).unwrap_or(i64::MAX), file_text))
}
