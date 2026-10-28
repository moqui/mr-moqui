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

//! `dev/Repositories` (plans/design-dev-aide.md 4.7): every root as `./moq git-status` lists it (kind, path,
//! components, branch, upstream, ahead and behind, changes, last fetch), Fetch all and Pull all root by root (one that
//! fails is reported in its row and the others go on), and Get a component through `./moq component-get` from the
//! names of `addons.xml` and `myaddons.xml` when the runtime has `moq`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use crate::audit::{self, Write};
use crate::git;
use crate::roots::RootKind;
use crate::tier::Tier;
use crate::workspace::{flag, json_value, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(list_repositories), list_repositories)?;
    registry.service_function(moqui_core::service_location!(fetch_all), fetch_all)?;
    registry.service_function(moqui_core::service_location!(pull_all), pull_all)?;
    registry.service_function(moqui_core::service_location!(get_addons), get_addons)?;
    registry.service_function(moqui_core::service_location!(get_component), get_component)
}

/// When `<git dir>/FETCH_HEAD` was last written, in seconds since the epoch: the root's last fetch.
fn last_fetch(context: &Context, path: &Path) -> Option<u64> {
    let git_dir = context.git().read(path, &["rev-parse", "--absolute-git-dir"]).ok()?;
    let modified = std::fs::metadata(PathBuf::from(git_dir.trim()).join("FETCH_HEAD"))
        .and_then(|meta| meta.modified())
        .ok()?;
    Some(modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs())
}

/// `list#Repositories`: each root with its git state.
fn list_repositories(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    _parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let git_available = context.git().available();
    let rows: Vec<Json> = context
        .roots
        .iter()
        .map(|root| {
            let mut row = json!({
                "rootId": root.id, "label": root.label, "kind": root.kind, "upstream": root.upstream,
                "path": root.path.display().to_string(),
                "components": root.components.iter().map(|component| component.name.as_str()).collect::<Vec<_>>().join(", "),
            });
            if root.kind == RootKind::Git && git_available {
                match context.git().read(
                    &root.path,
                    &["status", "--porcelain=v2", "-z", "--branch", "--untracked-files=normal"],
                ) {
                    Ok(out) => {
                        let status = git::parse_status(&out);
                        row["branch"] = json!(status.branch);
                        row["upstream"] = json!(status.upstream);
                        row["ahead"] = json!(status.ahead);
                        row["behind"] = json!(status.behind);
                        row["changes"] = json!(status.entries.iter().filter(|entry| entry.index != '!').count());
                        row["lastFetch"] = json!(last_fetch(&context, &root.path));
                    }
                    Err(err) => row["error"] = json!(err.to_string()),
                }
            }
            row
        })
        .collect();
    let mut out = Parameters::new();
    out.insert("repositories".into(), json_value(&rows));
    out.insert("gitAvailable".into(), Value::Bool(git_available));
    out.insert("moqAvailable".into(), Value::Bool(moq(&context).is_some()));
    Ok(out)
}

/// A network command run on every git root with an upstream, each by itself: its answer or its error in a row.
fn every_root(
    ec: &ExecutionContext,
    service: &ServiceInfo<'_>,
    run: fn(&ExecutionContext, &ServiceInfo<'_>, &Parameters) -> Result<Parameters, LibError>,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let mut results = Vec::new();
    let mut failed = 0_usize;
    let mut affected = 0_i64;
    for root in context.roots.iter().filter(|root| root.kind == RootKind::Git) {
        let mut parameters = Parameters::new();
        parameters.insert("rootId".into(), Value::string(&root.id));
        match run(ec, service, &parameters) {
            Ok(out) => {
                let output = out
                    .get("output")
                    .map(moqui_lib::object::to_plain_string)
                    .unwrap_or_default();
                if let Some(Value::I64(count)) = out.get("extensionsAffected") {
                    affected += count;
                }
                results.push(json!({ "rootId": root.id, "label": root.label, "ok": true,
                    "output": output.trim().lines().last().unwrap_or("") }));
            }
            Err(err) => {
                failed += 1;
                tracing::error!("Dev AIDE: {} failed in {}: {err}", service.name, root.label);
                results.push(json!({ "rootId": root.id, "label": root.label, "ok": false, "error": err.to_string() }));
            }
        }
    }
    let mut out = Parameters::new();
    out.insert("results".into(), json_value(&results));
    out.insert("failed".into(), Value::I64(i64::try_from(failed).unwrap_or(i64::MAX)));
    out.insert("extensionsAffected".into(), Value::I64(affected));
    Ok(out)
}

/// `fetch#All`.
fn fetch_all(
    ec: &ExecutionContext,
    service: &ServiceInfo<'_>,
    _parameters: &Parameters,
) -> Result<Parameters, LibError> {
    every_root(ec, service, crate::vcs::fetch_remote)
}

/// `pull#All`: fast-forward only, root by root, each with its reload and the site's extensions it broke.
fn pull_all(
    ec: &ExecutionContext,
    service: &ServiceInfo<'_>,
    _parameters: &Parameters,
) -> Result<Parameters, LibError> {
    every_root(ec, service, crate::vcs::pull_remote)
}

/// The runtime's `moq`, beside the runtime directory, when there is one.
fn moq(context: &Context) -> Option<PathBuf> {
    let script = context.factory.runtime_dir().parent()?.join("moq");
    script.is_file().then_some(script)
}

/// `get#Addons`: the components `addons.xml` and `myaddons.xml` name, each with whether it is here already.
fn get_addons(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    _parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let mut names: Vec<Json> = Vec::new();
    if let Some(base) = context.factory.runtime_dir().parent() {
        for file in ["addons.xml", "myaddons.xml"] {
            let Ok(body) = std::fs::read_to_string(base.join(file)) else {
                continue;
            };
            let Ok(root) = MNode::parse_shared(file, &body) else {
                tracing::warn!("Dev AIDE: {file} does not parse");
                continue;
            };
            for component in root.named_iter("component") {
                let Some(name) = component.attribute("name").filter(|name| !name.is_empty()) else {
                    continue;
                };
                if names.iter().any(|have| have["name"] == name.as_str()) {
                    continue;
                }
                let here = ["component", "dev-component"]
                    .iter()
                    .any(|dir| context.factory.runtime_dir().join(dir).join(&name).is_dir());
                names.push(json!({ "name": name, "here": here, "file": file }));
            }
        }
    }
    let mut out = Parameters::new();
    out.insert("addons".into(), json_value(&names));
    Ok(out)
}

/// `get#Component`: `./moq component-get <name>` (with `--dev` for `runtime/dev-component`), which gets the component
/// and those it depends on; it loads after a restart. The full tier's, with network git allowed.
fn get_component(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    let _authz = ec.disable_authz();
    crate::vcs::network_allowed(&context)?;
    let name = text(parameters, "name");
    if name.is_empty()
        || name.starts_with(['-', '.'])
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(LibError::new(format!("{name} is not a component name")));
    }
    let script =
        moq(&context).ok_or_else(|| LibError::new("This runtime has no ./moq beside it to get components with"))?;
    let mut command = Command::new(&script);
    command.arg("component-get");
    if flag(parameters, "dev") {
        command.arg("--dev");
    }
    command
        .arg(&name)
        .current_dir(script.parent().unwrap_or(Path::new(".")))
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null());
    let output = run_with_timeout(command, context.settings.git_network_timeout)?;
    audit::record(
        &context.facade,
        &Write {
            action: "DAA_GIT",
            root: "",
            path: &name,
            detail: Some(json!({ "command": "component-get", "dev": flag(parameters, "dev"), "ok": output.0 })),
            ..Write::default()
        },
    );
    if !output.0 {
        return Err(LibError::new(format!(
            "./moq component-get {name}: {}",
            output.1.trim()
        )));
    }
    let mut out = Parameters::new();
    out.insert("output".into(), Value::string(output.1));
    out.insert("restart".into(), Value::Bool(true));
    Ok(out)
}

/// Run `command`, killed past `timeout`: whether it succeeded and its output, both streams.
fn run_with_timeout(mut command: Command, timeout: Duration) -> Result<(bool, String), LibError> {
    let child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| LibError::new(format!("could not run ./moq: {err}")))?;
    let id = child.id();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(child.wait_with_output());
    });
    match receiver.recv_timeout(timeout) {
        Ok(Ok(output)) => Ok((
            output.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        )),
        Ok(Err(err)) => Err(LibError::new(format!("./moq: {err}"))),
        Err(_) => {
            #[cfg(unix)]
            let _ = Command::new("kill").arg(id.to_string()).status();
            #[cfg(not(unix))]
            let _ = id;
            Err(LibError::new(format!(
                "./moq ran past {} s and was stopped",
                timeout.as_secs()
            )))
        }
    }
}
