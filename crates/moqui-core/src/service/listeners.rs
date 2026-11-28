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

//! `org.moqui.admin.ListenerServices` (plans/Runtime-Config.md §3.6, RC4): `get#Listeners`, each listener with its state
//! and the settings behind it, for the `ADMIN` group; and `set#Listener`, one listener switched, moved or set, for a
//! holder of the sealed `LISTENER_CONTROL` permission who signed in recently. The work is
//! [`crate::listeners::rows`] and [`crate::listeners::change`], which the operator socket calls too; a refusal adds its
//! error and answers `reasonCode`, and `confirm` names the parameter that lets a guarded change through. A refusal is
//! on the audit log, as a change is. No Java counterpart.

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use crate::context::ExecutionContext;
use crate::entity::EntityFacade;
use crate::listeners::{Actor, Change, OWN_SETTINGS};
use crate::service::util::param_text;

/// Reading the listeners, as reading the conf, needs this group.
const ADMIN_GROUP: &str = "ADMIN";
/// Switching a listener needs this sealed permission too: apart from `CONF_EDIT`, so an incident responder switches
/// services without the right to edit the whole conf (§3.6).
const CONTROL_PERMISSION: &str = "LISTENER_CONTROL";

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, get_listeners)?;
    rust_service!(map, set_listener)?;
    Ok(())
}

fn add_error(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ec| ec.message().add_error(text));
}

/// The parameter `name` as a flag: a boolean, or `true` or `Y` as text; `default` when it is missing or empty.
fn flag(parameters: &BTreeMap<String, Value>, name: &str, default: bool) -> bool {
    match parameters.get(name) {
        Some(Value::Bool(flag)) => *flag,
        Some(value) if !is_empty(value) => {
            let text = to_plain_string(value);
            text.trim().eq_ignore_ascii_case("true") || text.trim().eq_ignore_ascii_case("y")
        }
        _ => default,
    }
}

/// The parameter `name`, trimmed, or none when it is missing or empty.
fn given(parameters: &BTreeMap<String, Value>, name: &str) -> Option<String> {
    Some(param_text(parameters, name).trim().to_string()).filter(|text| !text.is_empty())
}

/// A refusal's answer: `reasonCode`, and `confirm` when a confirmation lets the change through.
fn refused(code: &str, confirm: Option<&str>) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::from([("reasonCode".to_string(), Value::string(code))]);
    if let Some(confirm) = confirm {
        out.insert("confirm".to_string(), Value::string(confirm));
    }
    out
}

/// `get#Listeners`: `listenerList` ([`crate::listeners::rows`]), `running` (listeners run in this process), `origin`
/// (the listener the caller's request came over) and `recentChanges` (the changes people made since the server started,
/// newest first, [`crate::listeners::recent`]). Needs the `ADMIN` group.
fn get_listeners(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !crate::user::is_in_group(ADMIN_GROUP).unwrap_or(false) {
        add_error("Listing the listeners needs the ADMIN group");
        return Ok(BTreeMap::new());
    }
    let Some((factory, origin)) =
        ExecutionContext::with_current(|ec| (Arc::clone(ec.factory()), crate::listeners::origin(ec)))
    else {
        return Err(LibError::new("get#Listeners needs an execution context"));
    };
    let rows = crate::listeners::rows(&factory, origin.as_deref());
    let recent = crate::listeners::recent(&factory);
    Ok(BTreeMap::from([
        (
            "listenerList".to_string(),
            moqui_lib::json::from_json(&serde_json::Value::Array(rows)),
        ),
        (
            "recentChanges".to_string(),
            moqui_lib::json::from_json(&serde_json::Value::Array(recent)),
        ),
        (
            "running".to_string(),
            Value::Bool(crate::listeners::control(&factory).is_some()),
        ),
        ("origin".to_string(), origin.map_or(Value::Null, Value::string)),
    ]))
}

/// `set#Listener`: one listener turned on or off, with its port, host and own settings (`access` for SSH), as
/// [`crate::listeners::change`] checks and applies it. Needs the `ADMIN` group, the sealed `LISTENER_CONTROL` and a
/// recent sign-in, in that order; each refusal adds its error and answers `reasonCode` (`permission`,
/// `reauthRequired`, or the change's own: `invalid`, `portBusy`, `lockout`, `publicHttp`, `dependents`, `failed`) and,
/// for a guard, `confirm`. Out: `name`, `enabled`, `applied`, `message`, `settingList`, `outcomeList`, `noteList`.
fn set_listener(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some((factory, origin)) =
        ExecutionContext::with_current(|ec| (Arc::clone(ec.factory()), crate::listeners::origin(ec)))
    else {
        return Err(LibError::new("set#Listener needs an execution context"));
    };
    let name = param_text(parameters, "name").trim().to_string();
    let enabled = flag(parameters, "enabled", false);
    let reason = param_text(parameters, "reason").trim().to_string();
    let actor = Actor::User {
        username: crate::user::session::current_username()
            .ok()
            .flatten()
            .unwrap_or_default(),
        user_id: crate::user::current_user_id().ok().flatten(),
        origin,
    };
    let refuse = |code: &str, message: Option<String>| {
        crate::listeners::audit_refused(&name, &actor, Some(enabled), code, &reason);
        if let Some(message) = message {
            add_error(message);
        }
        Ok(refused(code, None))
    };
    if !crate::user::is_in_group(ADMIN_GROUP).unwrap_or(false) {
        return refuse(
            "permission",
            Some("Changing a listener needs the ADMIN group".to_string()),
        );
    }
    if !crate::user::has_permission(CONTROL_PERMISSION).unwrap_or(false) {
        return refuse(
            "permission",
            Some(format!("Changing a listener needs the {CONTROL_PERMISSION} permission")),
        );
    }
    if !super::recent_auth::require_recent_auth() {
        return refuse("reauthRequired", None);
    }
    // every own setting given, whichever listener it is named for: one given for another listener is refused
    let settings: BTreeMap<String, String> = OWN_SETTINGS
        .iter()
        .filter_map(|own| given(parameters, own.name).map(|value| (own.name.to_string(), value)))
        .collect();
    let change = Change {
        enabled,
        port: given(parameters, "port"),
        host: given(parameters, "host"),
        settings,
        reason,
        persist: flag(parameters, "persist", true),
        end_now: flag(parameters, "endNow", false),
        confirm_lockout: flag(parameters, "confirmLockout", false),
        confirm_public_http: flag(parameters, "confirmPublicHttp", false),
        confirm_dependents: flag(parameters, "confirmDependents", false),
        name,
    };
    match crate::listeners::change(&factory, &change, &actor) {
        Ok(answer) => match moqui_lib::json::from_json(&answer.to_json()) {
            Value::Map(fields) => Ok(fields.into_iter().collect()),
            _ => Err(LibError::new("set#Listener's answer is not a map")),
        },
        Err(refusal) => {
            add_error(refusal.message);
            Ok(refused(refusal.kind.code(), refusal.kind.confirmation()))
        }
    }
}
