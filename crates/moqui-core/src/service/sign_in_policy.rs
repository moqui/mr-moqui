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

//! `org.moqui.admin.ConfigurationServices.get#SignInPolicyValues`: the sign-in policy the server runs with, for the
//! delegated view of it (mr-boss plan/design-admin.md §6 SignInPolicy; impl-admin P10, F110). No Java counterpart.
//!
//! The password, login key and lockout rules are literal attributes of the conf's `user-facade` (`password`,
//! `login-key`, `login`) and the web session's of `webapp` `session-config`, which no `default-property` names, so a
//! script cannot read them and System › Conf, which shows the whole conf, is the `ADMIN` group's. This answers a fixed
//! list of them only, [`ATTRIBUTES`], each as the server reads it: the conf's value, else the fallback the code uses when
//! the conf leaves it out. None is a secret (the hash types, the seal lists and every key stay out). The keys are the
//! `confAttribute` of the `moqui.server.InstanceSetting` rows that describe them, which `get#EffectiveSettings` shows.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use crate::context::ExecutionContext;
use crate::entity::EntityFacade;

/// A read of the policy needs one of these: the configuration administrator's, or the access administrator's, who sets
/// the groups' sign-in rules beside it.
const PERMISSIONS: [&str; 4] = [
    "ADMIN_SETTINGS_VIEW",
    "ADMIN_SETTINGS_MANAGE",
    "ADMIN_ACCESS_VIEW",
    "ADMIN_ACCESS_MANAGE",
];
/// The web session's `webapp`.
const WEBAPP: &str = "webroot";

/// The attributes answered: the `user-facade` child (or `session-config` of the web session), the attribute, and the
/// value the code falls back to when the conf has none (`user/policy.rs`, `user/session.rs`).
const ATTRIBUTES: [(&str, &str, &str); 13] = [
    ("password", "min-length", "8"),
    ("password", "min-digits", "1"),
    ("password", "min-others", "1"),
    ("password", "history-limit", "5"),
    ("password", "change-weeks", "12"),
    ("password", "email-require-change", "false"),
    ("password", "email-expire-hours", "48"),
    ("login-key", "expire-hours", "144"),
    ("login-key", "device-expire-hours", "2160"),
    ("login", "max-failures", "3"),
    ("login", "disable-minutes", "5"),
    ("login", "history-store", "true"),
    ("session-config", "timeout", ""),
];

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, get_sign_in_policy_values)?;
    Ok(())
}

/// The key of an attribute, as a setting's `confAttribute` names it: `user-facade/password/@min-length`,
/// `webapp/session-config/@timeout`.
fn key(child: &str, name: &str) -> String {
    if child == "session-config" {
        format!("webapp/{child}/@{name}")
    } else {
        format!("user-facade/{child}/@{name}")
    }
}

/// `get#SignInPolicyValues`: `valueMap`, each of [`ATTRIBUTES`] by its key with the value in effect. An error without
/// one of [`PERMISSIONS`].
fn get_sign_in_policy_values(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let permitted = PERMISSIONS
        .iter()
        .any(|permission| crate::user::has_permission(permission).unwrap_or(false));
    if !permitted {
        return Err(LibError::new(format!(
            "{} or {} is required",
            PERMISSIONS[0], PERMISSIONS[2]
        )));
    }
    let values = ExecutionContext::with_current(|ctx| {
        let factory = ctx.factory();
        ATTRIBUTES
            .iter()
            .map(|(child, name, fallback)| {
                let value = if *child == "session-config" {
                    Some(factory.webapp_child_setting(WEBAPP, child, name)).filter(|value| !value.is_empty())
                } else {
                    factory
                        .user_facade_attr(child, name)
                        .map(|raw| moqui_lib::env::expand(&raw).unwrap_or(raw))
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty())
                };
                (
                    key(child, name),
                    Value::string(value.unwrap_or_else(|| (*fallback).to_string())),
                )
            })
            .collect::<moqui_lib::object::ValueMap>()
    })
    .unwrap_or_default();
    let mut out = BTreeMap::new();
    out.insert("valueMap".to_string(), Value::Map(values));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{key, ATTRIBUTES};

    #[test]
    fn the_policy_names_no_secret_and_each_key_once() {
        let keys: std::collections::BTreeSet<String> =
            ATTRIBUTES.iter().map(|(child, name, _)| key(child, name)).collect();
        assert_eq!(keys.len(), ATTRIBUTES.len());
        for (child, name, _) in ATTRIBUTES {
            assert!(
                !["hash", "key", "secret", "pass", "sealed", "privileged"]
                    .iter()
                    .any(|word| name.contains(word)),
                "{child} {name}"
            );
        }
        assert!(keys.contains("user-facade/password/@min-length") && keys.contains("webapp/session-config/@timeout"));
    }
}
