//! Component libraries in the two guided editors. Resolve the active component on the server;
//! only a full-tier interactive editor can start the checkout's package tool.
use std::path::{Path, PathBuf};
use std::process::Command;

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use serde_json::{json, Value};

use crate::roots::{Root, RootComponent};
use crate::tier::Tier;
use crate::workspace::{json_value, text, unfenced, Context};

fn io_error(error: std::io::Error) -> LibError {
    LibError::new(error.to_string())
}

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(get_libraries), get_libraries)?;
    registry.service_function(moqui_core::service_location!(manage_library), manage_library)
}

fn component<'a>(
    context: &'a Context,
    parameters: &Parameters,
) -> Result<(&'a Root, &'a RootComponent, PathBuf), LibError> {
    let root = context.root(&text(parameters, "rootId"))?;
    let path = context.resolve(root, &text(parameters, "path"))?;
    let component = root
        .component_of(&path.real)
        .ok_or_else(|| LibError::new("Choose a component file"))?;
    let loaded = context.factory.components();
    let active = loaded
        .iter()
        .find(|c| c.name == component.name)
        .ok_or_else(|| LibError::new("Choose a loaded component"))?;
    Ok((root, component, active.location.canonicalize().map_err(io_error)?))
}

fn active_checkout(root: &Root, component: &RootComponent, active: &Path) -> bool {
    root.worktree_of.is_none() && root.path.join(&component.path).canonicalize().ok().as_deref() == Some(active)
}

fn get_libraries(ec: &ExecutionContext, _: &ServiceInfo<'_>, parameters: &Parameters) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let (root, component, active) = component(&context, parameters)?;
    let file = crate::confine::resolve(&active, context.factory.runtime_dir(), "build/js-build.json")?.path;
    let config: Value = if file.exists() {
        serde_json::from_slice(&std::fs::read(file).map_err(io_error)?).map_err(|err| LibError::new(err.to_string()))?
    } else {
        json!({})
    };
    let can_manage = context.tier == Tier::Full
        && active_checkout(root, component, &active)
        && config["script"].is_null()
        && context.may_write(root, &text(parameters, "path")).is_ok();
    let data = json!({
        "component": component.name,
        "libraries": config["libraries"].as_array().cloned().unwrap_or_default(),
        "canManage": can_manage,
    });
    Ok([(String::from("result"), json_value(&data))].into())
}

fn manage_library(ec: &ExecutionContext, _: &ServiceInfo<'_>, parameters: &Parameters) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    unfenced(moqui_core::entity::fence::Effect::Process, "manage#Library")?;
    if moqui_core::screen::gate::interactive_request() != Some(moqui_core::screen::gate::InteractiveRequest::Browser) {
        return Err(LibError::new("Library management requires a signed-in browser session"));
    }
    let user = moqui_core::user::current_user_id()?.ok_or_else(|| LibError::new("Sign in to manage libraries"))?;
    let (root, component, active) = component(&context, parameters)?;
    if !active_checkout(root, component, &active) {
        return Err(LibError::new(
            "Choose a component of the running checkout to manage packages",
        ));
    }
    let path = text(parameters, "path");
    context.may_write(root, &path)?;
    let operation = text(parameters, "operation");
    let fields: &[&str] = match operation.as_str() {
        "search" => &["query"],
        "versions" => &["package"],
        "prepare" => &["libraryId", "entry", "version", "target"],
        "apply" => &["token"],
        _ => return Err(LibError::new("Unknown library operation")),
    };
    let args: Vec<_> = fields.iter().map(|key| text(parameters, key)).collect();
    if args
        .iter()
        .any(|arg| arg.is_empty() || arg.starts_with('-') || arg.len() > 256 || arg.chars().any(char::is_control))
    {
        return Err(LibError::new("Invalid library argument"));
    }
    let framework = context
        .factory
        .framework_component_dir()
        .canonicalize()
        .map_err(io_error)?;
    let checkout = framework
        .ancestors()
        .find(|dir| dir.join("scripts/moq/js.sh").is_file())
        .ok_or_else(|| LibError::new("Library management needs a framework source checkout"))?;
    // Start the script directly on Unix. On Windows pass user-supplied arguments straight to Git Bash,
    // without interpolating package searches into a cmd.exe command line.
    #[cfg(not(windows))]
    let mut command = Command::new(checkout.join("moq"));
    #[cfg(windows)]
    let mut command = {
        let bash = [
            ("ProgramFiles", "Git/bin/bash.exe"),
            ("ProgramFiles", "Git/usr/bin/bash.exe"),
            ("LocalAppData", "Programs/Git/bin/bash.exe"),
        ]
        .into_iter()
        .filter_map(|(var, path)| std::env::var_os(var).map(|dir| PathBuf::from(dir).join(path)))
        .find(|path| path.is_file())
        .ok_or_else(|| LibError::new("Install Git for Windows to manage libraries"))?;
        let mut command = Command::new(bash);
        command.arg(checkout.join("moq"));
        command
    };
    let output = command
        .arg("add-js")
        .arg(&operation)
        .arg(&component.name)
        .args(args)
        .current_dir(checkout)
        .env("MOQUI_RUNTIME", context.factory.runtime_dir())
        .env("JS_EXPECT_ROOT", &active)
        .env("JS_REVIEW_USER", user)
        .env("JS_VALIDATOR", std::env::current_exe().map_err(io_error)?)
        .env_remove("MOQ_JS_LOCKED")
        .output()
        .map_err(io_error)?;
    if !output.status.success() {
        return Err(LibError::new(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    if operation == "apply" {
        crate::audit::record(
            &context.facade,
            &crate::audit::Write {
                action: "DAA_SAVE",
                root: &root.id,
                path: &component.path,
                detail: Some(json!({"operation":"apply-library","review":text(parameters,"token")})),
                ..Default::default()
            },
        );
    }
    let result: Value = serde_json::from_slice(&output.stdout)
        .map_err(|err| LibError::new(format!("Reading package tool response: {err}")))?;
    Ok([(String::from("result"), json_value(&result))].into())
}
