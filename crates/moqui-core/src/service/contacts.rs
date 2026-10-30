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

//! `EmailClientServices.filter#ContactAddresses`: which senders are a user's contacts, for an automatic reply to
//! contacts only (`send#OutOfOfficeReplies`). The framework has no contacts of its own (they are the app's, such as
//! Mantle's parties), so it asks a hook service, as the shell counts do: the one the `email_contact_service` property
//! names, else the theme value `STRT_EMAIL_CONTACT_SERVICE` of the user's qapps theme. It is called with `userId` and
//! `emailAddressList` and answers `contactAddressList`, the addresses of those that are a contact the user can see. No
//! hook: `contactAddressList` is null, and the caller treats every sender as a contact. It is core's, not moqui-mail's,
//! so it answers with or without mail built in.

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use moqui_lib::sync::lock;

use crate::context::ExecutionContext;
use crate::entity::EntityFacade;
use crate::screen::sri::{theme_resource_values, user_theme_id, DEFAULT_QUASAR_THEME, QUASAR_THEME_TYPE};
use crate::service::rust_services::RustServiceFn;

/// The property that names the hook; it wins over the theme value.
const PROPERTY: &str = "email_contact_service";
/// The theme value that names the hook.
const THEME_VALUE: &str = "STRT_EMAIL_CONTACT_SERVICE";

pub(super) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, filter_contact_addresses)
}

/// The hooks that are missing or failed, each logged once while the server runs.
#[derive(Default)]
struct Logged(Mutex<HashSet<String>>);

fn first_time(key: &str) -> bool {
    ExecutionContext::with_current(|ec| {
        let logged = ec.factory().extension(|_| Logged::default());
        let first = lock(&logged.0).insert(key.to_string());
        first
    })
    .unwrap_or(true)
}

/// The lower-case addresses of a list, or of a comma separated text, without blanks and repeats.
fn addresses(value: Option<&Value>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |text: String| {
        let address = text.trim().to_lowercase();
        if !address.is_empty() && !out.contains(&address) {
            out.push(address);
        }
    };
    match value {
        Some(Value::List(items)) => {
            for item in items {
                if !is_empty(item) {
                    add(to_plain_string(item));
                }
            }
        }
        Some(other) if !is_empty(other) => {
            for part in to_plain_string(other).split(',') {
                add(part.to_string());
            }
        }
        _ => {}
    }
    out
}

/// The hook service for `user_id`: the property, else the user's qapps theme value.
fn hook_service(user_id: &str) -> Option<String> {
    let named = moqui_lib::env::get_prop_or_env(PROPERTY).trim().to_string();
    if !named.is_empty() {
        return Some(named);
    }
    let theme = user_theme_id(Some(user_id), QUASAR_THEME_TYPE, DEFAULT_QUASAR_THEME);
    match theme_resource_values(&theme, THEME_VALUE) {
        Ok(values) => values
            .into_iter()
            .map(|value| value.trim().to_string())
            .find(|value| !value.is_empty()),
        Err(err) => {
            tracing::error!("Could not read {THEME_VALUE} of theme {theme}: {err}");
            None
        }
    }
}

/// Call the hook as internal work (it decides for the user it is given), in a transaction of its own, so one that fails
/// leaves the caller's transaction as it was; an error it records is its failure, taken back out of the messages.
fn call_hook(
    name: &str,
    parameters: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, crate::context::Error> {
    let mark = ExecutionContext::with_current(|ec| ec.message().error_mark());
    let options = crate::service::call::CallOptions {
        disable_authz: true,
        require_new_transaction: Some(true),
        ..crate::service::call::CallOptions::default()
    };
    let result = crate::service::call::sync_with(name, parameters, &options);
    let errors: Vec<String> = mark
        .and_then(|mark| ExecutionContext::with_current(|ec| ec.message().take_errors_since(mark)))
        .unwrap_or_default();
    match result {
        Err(err) => Err(err),
        Ok(_) if !errors.is_empty() => Err(crate::context::Error::new(errors.join("; "))),
        Ok(out) => Ok(out),
    }
}

/// `filter#ContactAddresses`: of `emailAddressList`, those that are contacts of `userId` (the current user by default;
/// another user's only with authorization disabled). Out: `contactAddressList` (lower case; null when no hook is
/// configured), `hookService`, and `hookFailed` when the hook failed or answered no list, which is logged once by name
/// (then the list is empty: no address counts as a contact).
fn filter_contact_addresses(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let answer = |list: Option<Vec<String>>, service: Option<&str>, failed: bool| {
        BTreeMap::from([
            (
                "contactAddressList".to_string(),
                list.map_or(Value::Null, |list| {
                    Value::List(list.into_iter().map(Value::String).collect())
                }),
            ),
            ("hookService".to_string(), service.map_or(Value::Null, Value::string)),
            ("hookFailed".to_string(), Value::Bool(failed)),
        ])
    };
    let current = crate::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty());
    let given = crate::service::util::param_text(parameters, "userId");
    let Some(user_id) = Some(given).filter(|id| !id.is_empty()).or_else(|| current.clone()) else {
        return Ok(answer(None, None, false));
    };
    let internal = ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(true);
    if current.as_deref() != Some(user_id.as_str()) && !internal {
        ExecutionContext::with_current(|ec| {
            ec.message()
                .add_error("filter#ContactAddresses reads another user's contacts only with authorization disabled")
        });
        return Ok(BTreeMap::new());
    }
    let Some(service) = hook_service(&user_id) else {
        if first_time("none") {
            tracing::info!(
                "No {PROPERTY} property or {THEME_VALUE} theme value: every sender counts as a contact of an automatic reply"
            );
        }
        return Ok(answer(None, None, false));
    };
    let wanted = addresses(parameters.get("emailAddressList"));
    if wanted.is_empty() {
        return Ok(answer(Some(Vec::new()), Some(&service), false));
    }
    let call = BTreeMap::from([
        ("userId".to_string(), Value::string(&user_id)),
        (
            "emailAddressList".to_string(),
            Value::List(wanted.iter().map(Value::string).collect()),
        ),
    ]);
    let failed = |problem: String| {
        if first_time(&service) {
            tracing::error!("The contact service {service} {problem}; no sender counts as a contact");
        }
        Ok(answer(Some(Vec::new()), Some(&service), true))
    };
    match call_hook(&service, &call) {
        Ok(out) => match out.get("contactAddressList") {
            Some(list @ Value::List(_)) => {
                let known: HashSet<String> = addresses(Some(list)).into_iter().collect();
                // only the addresses asked about, in their order
                let found = wanted.into_iter().filter(|address| known.contains(address)).collect();
                Ok(answer(Some(found), Some(&service), false))
            }
            _ => failed("answered no contactAddressList".to_string()),
        },
        Err(err) => failed(format!("failed: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::addresses;
    use moqui_lib::object::Value;

    #[test]
    fn addresses_are_lower_case_without_blanks_or_repeats() {
        assert_eq!(
            addresses(Some(&Value::string(
                "Ada@Example.org, ,bob@example.com,ada@example.org"
            ))),
            vec!["ada@example.org".to_string(), "bob@example.com".to_string()]
        );
        assert_eq!(
            addresses(Some(&Value::List(vec![Value::string(" X@y.z "), Value::Null]))),
            vec!["x@y.z".to_string()]
        );
        assert!(addresses(None).is_empty());
    }
}
