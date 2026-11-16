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

//! `org.moqui.dev.InstanceServices.get#RegistryToken`: the registry token `init#InstanceDocker` sends with an image pull,
//! the first output line of a command the conf names (plans/Security-Review-Medium.md SEC-TLS-03, SEC-SCR-04,
//! SEC-OUT-08). An `InstanceImage.authTokenCmd` holds the name of an `instance-hosting` `token-command` of the conf, never
//! the command itself, so writing the row cannot choose a program; the conf is behind the sealed `CONF_EDIT`. The call
//! needs the sealed `INSTANCE_ADMIN`, as the instance services do. No script can start a process: the old
//! `ec.resource.runTokenCommand` is gone.

use std::collections::BTreeMap;
use std::time::Duration;

use moqui_lib::error::Error as LibError;
use moqui_lib::node::MNode;
use moqui_lib::object::{to_plain_string, Value};

use super::util::param_text;
use crate::context::ExecutionContext;
use crate::entity::EntityFacade;

/// The permission the call needs, as the instance services check it.
const PERMISSION: &str = "INSTANCE_ADMIN";
/// How long a token command may run.
const TIMEOUT: Duration = Duration::from_secs(30);

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, get_registry_token)?;
    Ok(())
}

fn add_error(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_error(text));
}

/// `get#RegistryToken`: run the conf's `token-command` that the image's `authTokenCmd` names and answer its first output
/// line as `authToken`. A refusal or a failure is a message error and no token.
fn get_registry_token(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let authz_disabled = ExecutionContext::with_current(|ctx| ctx.authz_disabled()).unwrap_or(false);
    if !authz_disabled && !crate::user::has_permission(PERMISSION)? {
        add_error(format!("Hosted instances need the {PERMISSION} permission"));
        return Ok(BTreeMap::new());
    }
    let image_id = param_text(parameters, "instanceImageId");
    let image = facade
        .find("moqui.server.instance.InstanceImage")?
        .disable_authz()
        .eq("instanceImageId", Value::string(&image_id))
        .one()?;
    let Some(image) = image else {
        add_error(format!("No InstanceImage {image_id}"));
        return Ok(BTreeMap::new());
    };
    let name = image.get("authTokenCmd").map(to_plain_string).unwrap_or_default();
    let name = name.trim();
    if name.is_empty() {
        return Ok(BTreeMap::new());
    }
    let conf = ExecutionContext::with_current(|ctx| ctx.factory().conf_text()).unwrap_or_default();
    let Some(command) = token_command(&conf, name) else {
        add_error(format!(
            "The token command {name} of image {image_id} is not a token-command of the conf's instance-hosting"
        ));
        return Ok(BTreeMap::new());
    };
    match run_first_line(&command) {
        Ok(token) => Ok(BTreeMap::from([("authToken".to_string(), Value::String(token))])),
        Err(err) => {
            tracing::error!("Registry token command {name} of image {image_id}: {}", err.message());
            add_error(format!("The registry token command {name} failed: {}", err.message()));
            Ok(BTreeMap::new())
        }
    }
}

/// The `command` of the `instance-hosting` `token-command` named `name` in the conf text, when there is one.
pub(crate) fn token_command(conf: &str, name: &str) -> Option<String> {
    let conf = MNode::parse_shared("moqui-conf", conf).ok()?;
    let hosting = conf.first_named("instance-hosting")?;
    hosting
        .children_named("token-command")
        .into_iter()
        .find(|node| node.attribute("name").as_deref() == Some(name))
        .and_then(|node| node.attribute("command"))
        .map(|command| moqui_lib::env::expand(&command).unwrap_or(command))
        .filter(|command| !command.trim().is_empty())
}

/// Run `command`, split on whitespace with no shell, stdin closed and only the named environment
/// (`moqui_lib::env::limit_child_env`), and answer the first line of its output, trimmed. A command that fails, or runs
/// past [`TIMEOUT`], is an error with its error output.
pub(crate) fn run_first_line(command: &str) -> Result<String, LibError> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut words = command.split_whitespace();
    let Some(program) = words.next() else {
        return Err(LibError::new("the token command is empty"));
    };
    crate::entity::fence::check_effect(crate::entity::fence::Effect::Process, program)?;
    let mut process = Command::new(program);
    moqui_lib::env::limit_child_env(&mut process, &[]);
    let mut child = process
        .args(words)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| LibError::new(format!("could not run token command {program}: {err}")))?;
    // Read both pipes on their own threads, so a child that writes much is not blocked on a full pipe.
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        })
    };
    let out = read(child.stdout.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
    let err = read(child.stderr.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(LibError::new(format!(
                    "token command {program} did not finish in {} seconds",
                    TIMEOUT.as_secs()
                )));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(wait_err) => return Err(LibError::new(format!("token command {program}: {wait_err}"))),
        }
    };
    let output = out.join().unwrap_or_default();
    let errors = err.join().unwrap_or_default();
    if !status.success() {
        return Err(LibError::new(format!(
            "token command {program} failed ({status}): {}",
            errors.trim()
        )));
    }
    Ok(output.lines().next().unwrap_or_default().trim().to_string())
}

#[cfg(test)]
#[path = "registry_token_tests.rs"]
mod tests;
