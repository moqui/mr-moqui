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

//! Scheduled sends and reminders in chat (plans/Comms-Messaging.md §18.3 phase 15): `run#AsSender`, the one piece of
//! `ChatScheduleServices` a script cannot do. A job sends a scheduled message, or brings a reminder, as its user at its
//! time, with the rules of that moment:
//!
//! - **The account.** Closed for good (`disabled` Y with no `disabledDateTime`) or terminated: `closed`. A timed lock (a
//!   sign-in lock, `disabledDateTime` within `login/@disable-minutes`): `locked`, which the caller tries again later and
//!   does not count; once the window is past it is no lock. A required or expired password change is a sign-in's
//!   business, not a job's: the user is set as [`crate::user::as_user`] sets it, with none of the sign-in's checks.
//! - **The app.** The screens the request that made the row went through (`screenList`, root first) are pushed as the
//!   user with authorization on, as a request to them would push them: a user who lost the app's grant is `noAccess`.
//! - **The call.** Then the service (one of [`ALLOWED`]) runs as the user with authorization off, as a screen's
//!   transition runs it under the screen's inherited grant; its errors come back as `errors`.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value};

use crate::context::{Authz, ExecutionContext, Tarpit};
use crate::entity::EntityFacade;

/// The services `run#AsSender` runs as a user.
pub const ALLOWED: [&str; 2] = [
    "org.moqui.chat.ChatServices.send#Message",
    "org.moqui.chat.ChatServices.get#RoomAccess",
];

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, run_as_sender)?;
    Ok(())
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .filter(|value| !value.is_null())
        .map(to_plain_string)
        .filter(|text| !text.is_empty())
}

/// What the account allows now (`ok`, `closed`, `locked`), from its row as `run#AsSender` reads it.
pub fn account_state(
    disabled: Option<&str>,
    disabled_at: Option<i64>,
    terminate_at: Option<i64>,
    now: i64,
    lock_minutes: i64,
) -> &'static str {
    if terminate_at.is_some_and(|at| at <= now) {
        return "closed";
    }
    if disabled != Some("Y") {
        return "ok";
    }
    match disabled_at {
        None => "closed",
        Some(at) if at + lock_minutes * 60_000 > now => "locked",
        // a sign-in lock whose window passed: the next sign-in clears it, and a send does not wait for one
        Some(_) => "ok",
    }
}

/// `run#AsSender`: `serviceName` (one of [`ALLOWED`]) with `parameters` as `userId`, after the account and app checks.
/// In: `userId`, `serviceName`, `parameters` (a Map), `screenList` (screen locations, root first), `nowMillis` (the tests'
/// clock). Out: `accountState` (`ok`, `closed`, `locked`), `reasonCode` (`noAccess` when the app's grant is gone),
/// `result` (the service's out-parameters), `errors` (its error texts, joined).
fn run_as_sender(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let user = text(parameters.get("userId")).ok_or_else(|| LibError::new("run#AsSender needs userId"))?;
    let service = text(parameters.get("serviceName")).unwrap_or_default();
    if !ALLOWED.contains(&service.as_str()) {
        return Err(LibError::new(format!("run#AsSender does not run {service}")));
    }
    let now = text(parameters.get("nowMillis"))
        .and_then(|text| text.parse::<i64>().ok())
        .unwrap_or_else(crate::user::now_millis);
    let mut out = BTreeMap::new();
    let account = {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        facade
            .find("moqui.security.UserAccount")?
            .eq("userId", Value::string(&user))
            .disable_authz()
            .one()?
    };
    let state = match &account {
        None => "closed",
        Some(row) => account_state(
            text(row.get("disabled")).as_deref(),
            row.get("disabledDateTime").and_then(crate::user::value_millis),
            row.get("terminateDate").and_then(crate::user::value_millis),
            now,
            crate::user::policy::disable_minutes(),
        ),
    };
    out.insert("accountState".to_string(), Value::string(state));
    if state != "ok" {
        return Ok(out);
    }
    let screens: Vec<String> = match parameters.get("screenList") {
        Some(Value::List(list)) => list
            .iter()
            .map(to_plain_string)
            .filter(|name| !name.is_empty())
            .collect(),
        _ => Vec::new(),
    };
    let call: BTreeMap<String, Value> = match parameters.get("parameters") {
        Some(Value::Map(map)) => map.iter().map(|(key, value)| (key.clone(), value.clone())).collect(),
        _ => BTreeMap::new(),
    };
    let ran = crate::user::as_user(&user, || {
        ExecutionContext::with_current(
            |ec| -> (Option<&'static str>, Option<BTreeMap<String, Value>>, String) {
                // the app's screens, root first, each checked as a request to it checks it
                let mut pushed = 0;
                let mut refused = false;
                for screen in &screens {
                    match ec.push_checked(
                        "AT_XML_SCREEN",
                        screen,
                        "AUTHZA_VIEW",
                        Authz::Required,
                        Tarpit::Skip,
                        None,
                    ) {
                        Ok(()) => pushed += 1,
                        Err(_) => {
                            refused = true;
                            break;
                        }
                    }
                }
                let answer = if refused {
                    (Some("noAccess"), None, String::new())
                } else {
                    let _authz = ec.disable_authz();
                    let result = crate::service::sync(&service, &call);
                    let mut errors = ec.message().errors().join("\n");
                    ec.message().clear_errors();
                    match result {
                        Ok(result) => (None, Some(result), errors),
                        Err(err) => {
                            if !errors.is_empty() {
                                errors.push('\n');
                            }
                            errors.push_str(err.message());
                            (None, None, errors)
                        }
                    }
                };
                for _ in 0..pushed {
                    if let Err(err) = ec.pop() {
                        tracing::warn!("run#AsSender: a screen frame of {user} was not popped: {err}");
                    }
                }
                answer
            },
        )
    })
    .flatten();
    let Some((reason, result, errors)) = ran else {
        out.insert("accountState".to_string(), Value::string("closed"));
        return Ok(out);
    };
    if let Some(reason) = reason {
        out.insert("reasonCode".to_string(), Value::string(reason));
    }
    if let Some(result) = result {
        out.insert("result".to_string(), Value::Map(result.into_iter().collect()));
    }
    out.insert("errors".to_string(), Value::string(errors));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::account_state;

    #[test]
    fn only_a_closed_or_terminated_account_is_closed_and_a_sign_in_lock_waits_its_window() {
        let now = 10_000_000;
        assert_eq!(account_state(None, None, None, now, 5), "ok");
        assert_eq!(account_state(Some("N"), None, None, now, 5), "ok");
        assert_eq!(account_state(Some("Y"), None, None, now, 5), "closed");
        assert_eq!(account_state(None, None, Some(now - 1), now, 5), "closed");
        assert_eq!(account_state(None, None, Some(now + 60_000), now, 5), "ok");
        assert_eq!(account_state(Some("Y"), Some(now - 60_000), None, now, 5), "locked");
        assert_eq!(
            account_state(Some("Y"), Some(now - 6 * 60_000), None, now, 5),
            "ok",
            "the lock's window passed"
        );
    }
}
