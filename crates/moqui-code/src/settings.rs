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

//! Settings (plans/agent-code/10-Web-UI.md section 9): one schema of names, types and defaults, ported from T3's
//! `ClientSettingsSchema` and `ServerSettings` (`service/org/moqui/code/settings-schema.json` in the component).
//!
//! - Client settings with store `user` are the user's `UserPreference` rows `code.<name>`, so they follow the user to
//!   each device. Store `device` ones stay on the device; the server only gives their defaults.
//! - Server settings are `AgentCodeSetting` rows; only an `AgentCodeAdmin` changes them.
//! - Policies are `AgentCodePolicy` rows (`limits.maxRuntimeMode`), changed by an admin only.
//!
//! A value is stored as JSON text. A name not in the schema is refused; a value of the wrong type is refused.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use moqui_core::entity::EntityFacade;
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde_json::{json, Map};

use crate::support::{json_text, text_of};

pub(crate) const SETTING: &str = "moqui.agent.code.AgentCodeSetting";
pub(crate) const POLICY: &str = "moqui.agent.code.AgentCodePolicy";

/// The `UserPreference` key prefix of the client settings.
pub const PREFERENCE_PREFIX: &str = "code.";

/// The largest stored value of a user setting (the `preferenceValue` field is 4000 characters).
pub const USER_VALUE_MAX: usize = 4000;

const SCHEMA_TEXT: &str = include_str!("../../../runtime/components/base/agent-code/service/org/moqui/code/settings-schema.json");

/// The parsed schema.
///
/// # Panics
///
/// When the schema file is not JSON (a test checks it).
pub fn schema() -> &'static serde_json::Value {
    static SCHEMA: OnceLock<serde_json::Value> = OnceLock::new();
    SCHEMA.get_or_init(|| serde_json::from_str(SCHEMA_TEXT).expect("settings-schema.json is JSON"))
}

/// The kinds of setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Client,
    Server,
    Policy,
}

impl Kind {
    const fn key(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Server => "server",
            Self::Policy => "policy",
        }
    }
}

fn spec(kind: Kind, name: &str) -> Option<&'static serde_json::Value> {
    schema().get(kind.key())?.get(name)
}

/// Why a setting value is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingError(pub String);

impl std::fmt::Display for SettingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Check `value` against the schema entry of `name`.
///
/// # Errors
///
/// A name not in the schema, or a value of another type or outside its range.
pub fn check(kind: Kind, name: &str, value: &serde_json::Value) -> Result<(), SettingError> {
    let spec = spec(kind, name).ok_or_else(|| SettingError(format!("{name} is not a {} setting", kind.key())))?;
    let bad = || SettingError(format!("{value} is not a value of {name}"));
    let number_in_range = |n: f64| {
        let min = spec.get("min").and_then(serde_json::Value::as_f64);
        let max = spec.get("max").and_then(serde_json::Value::as_f64);
        min.is_none_or(|min| n >= min) && max.is_none_or(|max| n <= max)
    };
    let ok = match spec.get("type").and_then(serde_json::Value::as_str).unwrap_or("any") {
        "boolean" => value.is_boolean(),
        "enum" => value.as_str().is_some_and(|text| {
            spec.get("values")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|values| values.iter().any(|v| v.as_str() == Some(text)))
        }),
        "number" => value.as_f64().is_some_and(number_in_range),
        "integer" => value.as_i64().is_some_and(|n| {
            #[allow(clippy::cast_precision_loss)]
            number_in_range(n as f64)
        }),
        "string" => value.as_str().is_some_and(|text| {
            let max = spec.get("maxLength").and_then(serde_json::Value::as_u64).unwrap_or(4096);
            u64::try_from(text.chars().count()).unwrap_or(u64::MAX) <= max
        }),
        "list" => value.is_array(),
        "record" => value.is_object(),
        _ => true,
    };
    if ok {
        Ok(())
    } else {
        Err(bad())
    }
}

/// The default of `name`, `Null` when it has none.
#[must_use]
pub fn default_of(kind: Kind, name: &str) -> serde_json::Value {
    spec(kind, name)
        .and_then(|spec| spec.get("default"))
        .cloned()
        .unwrap_or(serde_json::Value::Null)
}

/// The store of a client setting: `user` or `device`.
#[must_use]
pub fn store_of(name: &str) -> Option<&'static str> {
    spec(Kind::Client, name)?.get("store")?.as_str()
}

fn stored(text: Option<String>) -> Option<serde_json::Value> {
    text.and_then(|text| serde_json::from_str(&text).ok())
}

fn row_value(facade: &EntityFacade, entity: &str, key_field: &str, value_field: &str, name: &str) -> Option<serde_json::Value> {
    let row = facade
        .find(entity)
        .and_then(|find| find.eq(key_field, Value::string(name)).disable_authz().one())
        .map_err(|err| tracing::warn!("agent code: setting {name} was not read: {err}"))
        .ok()
        .flatten()?;
    stored(text_of(&row, value_field))
}

/// A server setting: the stored one, else the default.
#[must_use]
pub fn server_value(facade: &EntityFacade, name: &str) -> serde_json::Value {
    row_value(facade, SETTING, "settingName", "settingValue", name).unwrap_or_else(|| default_of(Kind::Server, name))
}

/// A policy: the stored one, else the default.
#[must_use]
pub fn policy_value(facade: &EntityFacade, name: &str) -> serde_json::Value {
    row_value(facade, POLICY, "policyKey", "policyValue", name).unwrap_or_else(|| default_of(Kind::Policy, name))
}

/// Every server setting with its value.
#[must_use]
pub fn server_settings(facade: &EntityFacade) -> Map<String, serde_json::Value> {
    let mut stored_rows = BTreeMap::new();
    match facade.find(SETTING).and_then(|find| find.disable_authz().list()) {
        Ok(rows) => {
            for row in rows.iter() {
                if let (Some(name), Some(value)) = (text_of(row, "settingName"), stored(text_of(row, "settingValue"))) {
                    stored_rows.insert(name, value);
                }
            }
        }
        Err(err) => tracing::warn!("agent code: the server settings were not read: {err}"),
    }
    let names = schema().get("server").and_then(serde_json::Value::as_object);
    names
        .into_iter()
        .flatten()
        .map(|(name, _)| {
            let value = stored_rows
                .remove(name)
                .unwrap_or_else(|| default_of(Kind::Server, name));
            (name.clone(), value)
        })
        .collect()
}

/// Store `value` for server setting `name` (`Null` restores the default).
///
/// # Errors
///
/// A refused value, or the write failing.
pub fn set_server(facade: &EntityFacade, name: &str, value: &serde_json::Value, user_id: &str) -> Result<(), Error> {
    set_row(facade, Kind::Server, SETTING, ("settingName", "settingValue"), name, value, Some(user_id))
}

/// Store `value` for policy `name` (`Null` restores the default).
///
/// # Errors
///
/// A refused value, or the write failing.
pub fn set_policy(facade: &EntityFacade, name: &str, value: &serde_json::Value) -> Result<(), Error> {
    set_row(facade, Kind::Policy, POLICY, ("policyKey", "policyValue"), name, value, None)
}

fn set_row(
    facade: &EntityFacade,
    kind: Kind,
    entity: &str,
    (key_field, value_field): (&str, &str),
    name: &str,
    value: &serde_json::Value,
    user_id: Option<&str>,
) -> Result<(), Error> {
    let find = facade.find(entity)?.eq(key_field, Value::string(name)).disable_authz();
    if value.is_null() {
        find.delete_all()?;
        return Ok(());
    }
    check(kind, name, value).map_err(|err| Error::new(err.0))?;
    let mut row = facade.make_value(entity)?;
    row.set(key_field, Value::string(name));
    row.set(value_field, Value::string(json_text(value)));
    if let Some(user_id) = user_id {
        row.set("lastUpdatedByUserId", Value::string(user_id));
    }
    row.create_or_update(facade)
}

/// The client settings of the current user: user ones stored or default, device ones default.
#[must_use]
pub fn client_settings() -> Map<String, serde_json::Value> {
    let preferences = moqui_core::user::get_preferences().unwrap_or_default();
    let names = schema().get("client").and_then(serde_json::Value::as_object);
    names
        .into_iter()
        .flatten()
        .map(|(name, spec)| {
            let user_store = spec.get("store").and_then(serde_json::Value::as_str) == Some("user");
            let value = user_store
                .then(|| stored(preferences.get(&format!("{PREFERENCE_PREFIX}{name}")).cloned()))
                .flatten()
                .unwrap_or_else(|| default_of(Kind::Client, name));
            (name.clone(), value)
        })
        .collect()
}

/// Store the current user's client settings in `patch` (`Null` restores a default). All are checked before any is
/// stored.
///
/// # Errors
///
/// A name not in the schema, a device setting, a value of the wrong type or too long, or a write failing.
pub fn set_client(patch: &Map<String, serde_json::Value>) -> Result<(), Error> {
    let mut writes = Vec::new();
    for (name, value) in patch {
        if store_of(name) == Some("device") {
            return Err(Error::new(format!("{name} is kept on each device, not on the server")));
        }
        if value.is_null() {
            writes.push((name, None));
            continue;
        }
        check(Kind::Client, name, value).map_err(|err| Error::new(err.0))?;
        let text = json_text(value);
        if text.chars().count() > USER_VALUE_MAX {
            return Err(Error::new(format!("the value of {name} is over {USER_VALUE_MAX} characters")));
        }
        writes.push((name, Some(text)));
    }
    for (name, text) in writes {
        moqui_core::user::set_preference(&format!("{PREFERENCE_PREFIX}{name}"), text.as_deref())?;
    }
    Ok(())
}

/// The schema as the settings pages read it.
#[must_use]
pub fn schema_json() -> serde_json::Value {
    json!({
        "client": schema().get("client"),
        "server": schema().get("server"),
        "policy": schema().get("policy"),
    })
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
