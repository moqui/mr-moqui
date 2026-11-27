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

//! Rebuild from the app (plans/archive/design-dev-aide.md 6.5): `./moq build` of the package this server runs, typed into a new
//! host session of the Terminal in the checkout, as Run locally types its script (`open#TerminalSession` `input`). The
//! shell `exec`s it (sh, bash, zsh, fish, csh and tcsh all take `exec`), so the session's end is the build's: the Terminal keeps an
//! ended session listed with its exit status and its last output (`find#TerminalSessions`), and the banner that follows
//! it offers the restart (`restart#Server`) once it ended with 0. Offered only where the runtime has `moq` beside it, as
//! Repositories' "Get a component" is.

use std::path::Path;

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use crate::audit::{self, Write};
use crate::tier::Tier;
use crate::workspace::{text, Context};

/// The user preference that keeps the session of the user's last rebuild, which the banner follows.
const LAST_SESSION: &str = "DEV_AIDE_REBUILD_SESSION";

/// The title of a rebuild's session in the Terminal.
const TITLE: &str = "Rebuild: ./moq build";

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(rebuild_server), rebuild_server)?;
    registry.service_function(moqui_core::service_location!(get_rebuild), get_rebuild)
}

/// The `./moq build` package option of the binary at `exe` when it is one the checkout at `checkout` built
/// (`target/<package>/moqui`): `--debug`, `--lite` or `--enterprise`, none for release (moq's default) or for a binary
/// built elsewhere.
pub fn package_option(exe: &Path, checkout: &Path) -> Option<&'static str> {
    let dir = exe.parent()?;
    if dir.parent()? != checkout.join("target") {
        return None;
    }
    match dir.file_name()?.to_str()? {
        "debug" => Some("--debug"),
        "lite" => Some("--lite"),
        "enterprise" => Some("--enterprise"),
        _ => None,
    }
}

/// The command line typed into the session: `exec ./moq build` with the package option, so moq takes the shell's place
/// and the session ends with the build's status, whatever the user's shell (a `; exit $?` is not fish's or csh's).
pub fn command_line(package: Option<&str>) -> String {
    match package {
        Some(option) => format!("exec ./moq build {option}\n"),
        None => "exec ./moq build\n".to_string(),
    }
}

/// Whether this platform has host sessions to rebuild in: the Terminal's are Unix only.
const HOST_SESSIONS: bool = cfg!(unix);

/// What the banner shows of a rebuild's session: `running`, `succeeded` (ended with 0), `failed` (ended otherwise) or
/// `gone` (no longer listed: ended long ago, or by a restart).
pub fn state_of(session: Option<&Json>) -> &'static str {
    match session {
        None => "gone",
        Some(session) if session["state"] == "ended" => {
            if session["exitStatus"].as_i64() == Some(0) {
                "succeeded"
            } else {
                "failed"
            }
        }
        Some(_) => "running",
    }
}

/// `rebuild#Server`: a new host session of the Terminal in the checkout with `./moq build` typed in, for the package
/// this server runs. The full tier's, audited; a person's from a session (never an agent's fence, a sim or a header
/// login). The Terminal decides who may open one; its refusal comes back as `reasonCode`.
fn rebuild_server(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    _parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    crate::workspace::unfenced(moqui_core::entity::fence::Effect::Process, "rebuild#Server")?;
    if moqui_core::screen::gate::header_authenticated_request() {
        return Err(LibError::new(
            "Rebuild is a person's, from a session: not a request logged in by a header",
        ));
    }
    if !HOST_SESSIONS {
        return Err(LibError::new(
            "Rebuild runs in a host session of the Terminal, which this platform does not have: run ./moq build",
        ));
    }
    let program =
        crate::repos::moq(&context).ok_or_else(|| LibError::new("This runtime has no moq beside it to build with"))?;
    let checkout = program.parent().unwrap_or(Path::new(".")).to_path_buf();
    let package = std::env::current_exe()
        .ok()
        .and_then(|exe| package_option(&exe, &checkout));
    let input = command_line(package);
    let opened = moqui_core::service::sync(
        "org.moqui.terminal.TerminalServices.open#TerminalSession",
        &[
            ("kind", "host".to_string()),
            // the server user's shell, as Run locally's
            ("profileName", "shell".to_string()),
            ("directory", checkout.display().to_string()),
            ("title", TITLE.to_string()),
            ("input", input.clone()),
            ("cols", "120".to_string()),
            ("rows", "32".to_string()),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), Value::string(value)))
        .collect(),
    )?;
    let session_id = opened
        .get("sessionId")
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default();
    // a refusal of the Terminal opened nothing: the audit says what did not run, and why
    let refused = opened.get("reasonCode").map(moqui_lib::object::to_plain_string);
    audit::record(
        &context.facade,
        &Write {
            action: "DAA_BUILD",
            root: "",
            path: "moq build",
            detail: Some(
                json!({ "command": input.trim_end(), "directory": checkout.display().to_string(),
                "sessionId": session_id, "opened": !session_id.is_empty(), "refused": refused }),
            ),
            ..Write::default()
        },
    );
    if !session_id.is_empty() {
        moqui_core::user::set_preference(LAST_SESSION, Some(&session_id))?;
    }
    let mut out = Parameters::new();
    for key in ["sessionId", "reasonCode"] {
        if let Some(value) = opened.get(key) {
            out.insert(key.into(), value.clone());
        }
    }
    out.insert("command".into(), Value::string(input.trim_end()));
    Ok(out)
}

/// `get#Rebuild`: whether this runtime can rebuild (`available`: it has `moq`, and the user the full tier), and the
/// user's rebuild session (`sessionId`, else their last one), with its `state` ([`state_of`]) and `exitStatus`.
fn get_rebuild(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let mut out = Parameters::new();
    out.insert(
        "available".into(),
        Value::Bool(HOST_SESSIONS && context.tier >= Tier::Full && crate::repos::moq(&context).is_some()),
    );
    let asked = text(parameters, "sessionId");
    let session_id = if asked.is_empty() {
        moqui_core::user::get_preference(LAST_SESSION)?.unwrap_or_default()
    } else {
        asked
    };
    if session_id.is_empty() {
        return Ok(out);
    }
    // a binary without the Terminal lists no session, so the one asked for is gone
    let sessions = match moqui_core::service::sync(
        "org.moqui.terminal.TerminalServices.find#TerminalSessions",
        &Parameters::new(),
    ) {
        Ok(found) => found
            .get("sessions")
            .map(moqui_lib::json::to_json)
            .unwrap_or(Json::Null),
        Err(err) => {
            tracing::warn!("get#Rebuild: the Terminal's sessions were not read: {err}");
            Json::Null
        }
    };
    let session = sessions
        .as_array()
        .and_then(|list| list.iter().find(|session| session["id"] == session_id.as_str()));
    out.insert("sessionId".into(), Value::string(&session_id));
    out.insert("state".into(), Value::string(state_of(session)));
    if let Some(status) = session.and_then(|session| session["exitStatus"].as_i64()) {
        out.insert("exitStatus".into(), Value::I64(status));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_package_of_the_running_binary() {
        let checkout = Path::new("/w/mr-moqui");
        assert_eq!(
            package_option(Path::new("/w/mr-moqui/target/debug/moqui"), checkout),
            Some("--debug")
        );
        assert_eq!(
            package_option(Path::new("/w/mr-moqui/target/lite/moqui"), checkout),
            Some("--lite")
        );
        assert_eq!(
            package_option(Path::new("/w/mr-moqui/target/enterprise/moqui"), checkout),
            Some("--enterprise")
        );
        assert_eq!(
            package_option(Path::new("/w/mr-moqui/target/release/moqui"), checkout),
            None
        );
        // a binary another checkout or an archive holds: moq's default package
        assert_eq!(package_option(Path::new("/w/other/target/debug/moqui"), checkout), None);
        assert_eq!(package_option(Path::new("/opt/moqui/moqui"), checkout), None);
    }

    #[test]
    fn the_shell_ends_with_the_build() {
        // moq takes the shell's place, in any shell, so the session's exit status is the build's
        assert_eq!(command_line(None), "exec ./moq build\n");
        assert_eq!(command_line(Some("--debug")), "exec ./moq build --debug\n");
        assert_eq!(command_line(Some("--lite")), "exec ./moq build --lite\n");
    }

    #[test]
    fn a_session_as_the_banner_shows_it() {
        assert_eq!(state_of(None), "gone");
        assert_eq!(
            state_of(Some(&json!({"state": "running", "exitStatus": null}))),
            "running"
        );
        assert_eq!(state_of(Some(&json!({"state": "ended", "exitStatus": 0}))), "succeeded");
        assert_eq!(state_of(Some(&json!({"state": "ended", "exitStatus": 101}))), "failed");
    }
}
