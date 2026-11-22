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

//! The bodies of the `type="rust"` services of `org.moqui.code` (component `base/agent-code`): the command
//! dispatcher, the shell snapshot, the settings, the descriptor, and the effect worker with the feed cleanup.

use std::collections::BTreeMap;

use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_core::rust_service;
use moqui_lib::error::Error as LibError;
use moqui_lib::json::{from_json, to_json};
use moqui_lib::object::Value;
use moqui_db::sql::Condition;

use crate::support::{add_error, current_user, has_permission, now_millis, out, stamp, text, Parameters};
use crate::{command, effect, environment, settings, shell, ADMIN_PERMISSION, USE_PERMISSION};

type Out = Result<BTreeMap<String, Value>, LibError>;

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, dispatch_command)?;
    rust_service!(map, get_shell_snapshot)?;
    rust_service!(map, get_settings)?;
    rust_service!(map, update_settings)?;
    rust_service!(map, get_descriptor)?;
    rust_service!(map, run_effects)?;
    rust_service!(map, clean_feed)
}

/// The current user when they hold `permission`; else an error on the context and `None`.
fn user_with(permission: &str) -> Option<String> {
    let Some(user_id) = current_user() else {
        add_error("Agent Code needs a logged-in user");
        return None;
    };
    if !has_permission(permission) {
        add_error(format!("this needs the {permission} permission"));
        return None;
    }
    Some(user_id)
}

fn json_param(parameters: &Parameters, key: &str) -> serde_json::Value {
    match parameters.get(key) {
        Some(Value::String(text)) => serde_json::from_str(text).unwrap_or(serde_json::Value::Null),
        Some(value) => to_json(value),
        None => serde_json::Value::Null,
    }
}

fn runtime_dir() -> std::path::PathBuf {
    ExecutionContext::with_current(|ec| ec.factory().runtime_dir().to_path_buf()).unwrap_or_default()
}

/// `CommandServices.dispatch#Command`: one command (02 section 1). Internal commands are accepted only when the
/// caller passes `internal` true, which a client cannot: the service is `allow-remote="false"`.
fn dispatch_command(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(user_id) = user_with(USE_PERMISSION) else {
        return Ok(BTreeMap::new());
    };
    let command = json_param(parameters, "command");
    let internal = text(parameters, "internal").as_deref() == Some("true");
    match command::dispatch(facade, &user_id, &command, internal) {
        Ok(receipt) => Ok(out([
            ("sequence", receipt.sequence.map_or(Value::Null, Value::I64)),
            ("result", from_json(&receipt.result)),
            ("replayed", Value::Bool(receipt.replayed)),
        ])),
        Err(err) => {
            add_error(err.to_string());
            Ok(out([("errorCode", Value::string(err.code))]))
        }
    }
}

/// `ShellServices.get#ShellSnapshot`: the user's sidebar and the change number it is at.
fn get_shell_snapshot(_parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(user_id) = user_with(USE_PERMISSION) else {
        return Ok(BTreeMap::new());
    };
    let snapshot = shell::snapshot(facade, &user_id)?;
    Ok(out([("snapshot", from_json(&snapshot))]))
}

/// `SettingsServices.get#Settings`: `kind` client (the user's), server or schema.
fn get_settings(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if user_with(USE_PERMISSION).is_none() {
        return Ok(BTreeMap::new());
    }
    let settings = match text(parameters, "kind").as_deref().unwrap_or("client") {
        "client" => serde_json::Value::Object(settings::client_settings()),
        "server" => serde_json::Value::Object(settings::server_settings(facade)),
        "schema" => settings::schema_json(),
        other => {
            add_error(format!("{other} is not a settings kind: client, server or schema"));
            return Ok(BTreeMap::new());
        }
    };
    Ok(out([("settings", from_json(&settings))]))
}

/// `SettingsServices.update#Settings`: a `patch` map of names to values; `Null` restores a default. Server settings
/// and policies need `AgentCodeAdmin`.
fn update_settings(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let kind = text(parameters, "kind").unwrap_or_else(|| "client".into());
    let permission = if kind == "client" { USE_PERMISSION } else { ADMIN_PERMISSION };
    let Some(user_id) = user_with(permission) else {
        return Ok(BTreeMap::new());
    };
    let patch = json_param(parameters, "patch");
    let Some(patch) = patch.as_object() else {
        add_error("patch is a map of setting names to values");
        return Ok(BTreeMap::new());
    };
    let written = match kind.as_str() {
        "client" => settings::set_client(patch),
        "server" | "policy" => crate::support::in_tx(facade, settings::SETTING, || {
            for (name, value) in patch {
                if kind == "server" {
                    settings::set_server(facade, name, value, &user_id)?;
                } else {
                    settings::set_policy(facade, name, value)?;
                }
            }
            Ok(())
        }),
        other => Err(LibError::new(format!("{other} is not a settings kind: client, server or policy"))),
    };
    if let Err(err) = written {
        add_error(err.to_string());
    }
    Ok(BTreeMap::new())
}

/// `EnvironmentServices.get#Descriptor`: the descriptor of the well-known route (no user data).
fn get_descriptor(_parameters: &Parameters, facade: &EntityFacade) -> Out {
    let descriptor = environment::descriptor(facade, &runtime_dir())?;
    Ok(out([("descriptor", from_json(&descriptor))]))
}

fn node_name() -> String {
    ExecutionContext::with_current(|ec| moqui_core::cluster::instance_id(ec.factory()))
        .flatten()
        .unwrap_or_else(|| format!("pid-{}", std::process::id()))
}

/// `EffectServices.run#Effects`: one pass of the effect worker (the `agent_code_effects` job, and the wake after a
/// commit).
fn run_effects(_parameters: &Parameters, facade: &EntityFacade) -> Out {
    let pass = effect::run_due(facade, &node_name())?;
    Ok(out([
        ("done", Value::I64(i64::try_from(pass.done).unwrap_or_default())),
        ("retried", Value::I64(i64::try_from(pass.retried).unwrap_or_default())),
        ("failed", Value::I64(i64::try_from(pass.failed).unwrap_or_default())),
    ]))
}

/// `EffectServices.clean#AgentCodeFeed`: done effects after 7 days, receipts after 7 days, changes after 1 day. Each
/// part logs its own failure and the others go on.
fn clean_feed(_parameters: &Parameters, facade: &EntityFacade) -> Out {
    let effects = effect::clean(facade)
        .map_err(|err| tracing::error!("agent code: the done effects were not cleaned: {err}"))
        .unwrap_or_default();
    let receipts = command::clean_receipts(facade)
        .map_err(|err| tracing::error!("agent code: the command receipts were not cleaned: {err}"))
        .unwrap_or_default();
    let before = now_millis() - 24 * 60 * 60 * 1000;
    let changes = facade
        .find(crate::change::CHANGE)
        .and_then(|find| {
            find.condition(Condition::less("changeDate", stamp(before)))
                .disable_authz()
                .delete_all()
        })
        .map_err(|err| tracing::error!("agent code: the old changes were not cleaned: {err}"))
        .unwrap_or_default();
    let count = |n: usize| Value::I64(i64::try_from(n).unwrap_or_default());
    Ok(out([
        ("effects", count(effects)),
        ("receipts", count(receipts)),
        ("changes", Value::I64(changes)),
    ]))
}
