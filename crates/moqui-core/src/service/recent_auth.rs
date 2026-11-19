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

//! `org.moqui.sso.RecentAuthServices`: the recent sign-in check of sensitive actions and the re-authentication that
//! satisfies it. No Java counterpart.
//!
//! - `check#RecentAuth`: whether the session's user signed in within `maxAgeSeconds` (5 minutes by default), as
//!   [`crate::user::recent_auth`] records it (a password login, the code of a second factor, an SSO login, a
//!   re-authentication; never a key, an app password or an internal login), and how the user confirms who they are
//!   again: their password (and the second factor when the account has one), or a fresh SSO sign-in for an account
//!   without a password.
//! - `reauthenticate#User`: the password, then the code, of the signed-in user ([`crate::user::reauthenticate`]).
//! - `send#ReauthCode`: a code to an email or SMS factor, once the password was checked.
//!
//! - `require#RecentAuth`: the same rule as a refusal, for the services that make or end a credential or a sign-in
//!   factor of the caller's own account, and for an administrator's action on another account (its password, codes,
//!   factors, email or provider link, its credentials revoked, Login as) ([`require_recent_auth`]).
//!
//! The ChatKeys services refuse a cross-signing reset, a new or deleted key backup and a device revoke without it
//! (`reauthRequired`); the browser's piece is `js/WebrootReAuth.js` over the `/ReAuth` transitions, and a form refused for
//! it ([`REAUTH_HEADER`]) asks there and sends again.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use super::util::param_text;
use crate::context::ExecutionContext;
use crate::entity::{EntityFacade, EntityValue};
use crate::user::CodeLogin;

/// The age a recent sign-in may have, unless the caller names another.
const DEFAULT_MAX_AGE_SECONDS: i64 = 300;
/// The session attribute an SSO login keeps its flow in (`service/sso.rs`).
const SSO_FLOW_ID: &str = "moquiAuthFlowId";
/// The response header of a refusal for want of a recent sign-in ([`require_recent_auth`]): a browser form that gets it
/// asks the user to confirm who they are (`moqui.reauth`) and sends again (`js/WebrootVue.qvt.js` m-form).
pub(crate) const REAUTH_HEADER: &str = "X-Moqui-Reauth";
/// The error of that refusal, as the ChatKeys services word it.
const REAUTH_TEXT: &str = "Confirm it is you: sign in again to do this";

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, check_recent_auth)?;
    rust_service!(map, reauthenticate_user)?;
    rust_service!(map, send_reauth_code)?;
    rust_service!(map, require_recent_auth_service)?;
    Ok(())
}

/// The rule of an action that makes or ends a credential or a sign-in factor of the caller's own account (SEC-ADM-03,
/// SEC-ADM-07): the session's user signed in, or confirmed who they are, in the last five minutes
/// ([`crate::user::recent_auth`]: a password, a second factor's code, an SSO sign-in, `reauthenticate#User`; never a key, an
/// app password or an internal login), so a stolen session or key cannot make itself lasting credentials or take the
/// account's factors. A caller that disabled authorization (a job, a setup service) answers for the action and passes.
/// False when refused: the error asking the user to confirm who they are is added and the response carries
/// [`REAUTH_HEADER`].
pub(crate) fn require_recent_auth() -> bool {
    if ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false)
        || crate::user::recent_auth(DEFAULT_MAX_AGE_SECONDS * 1000)
    {
        return true;
    }
    add_error(localized(REAUTH_TEXT));
    crate::web::set_header(REAUTH_HEADER, Some("required"));
    false
}

/// `require#RecentAuth`: [`require_recent_auth`] for an XML service. Out: `recent`, and `reasonCode` `reauthRequired`
/// when refused.
fn require_recent_auth_service(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let recent = require_recent_auth();
    let mut out = BTreeMap::from([("recent".to_string(), Value::Bool(recent))]);
    if !recent {
        out.insert("reasonCode".to_string(), Value::string("reauthRequired"));
    }
    Ok(out)
}

fn add_error(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_error(text));
}

fn localized(text: &str) -> String {
    crate::l10n::localize(text).unwrap_or_else(|_| text.to_string())
}

fn field(row: &EntityValue, name: &str) -> String {
    row.get(name)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .unwrap_or_default()
}

fn truthy(parameters: &BTreeMap<String, Value>, name: &str) -> bool {
    match parameters.get(name) {
        Some(Value::Bool(flag)) => *flag,
        Some(other) => matches!(to_plain_string(other).as_str(), "true" | "Y" | "y"),
        None => false,
    }
}

/// `check#RecentAuth`: `recent`, `authAgeSeconds` (none when the session's user never signed in that way), `method`
/// (`password`, `sso`, or `none` with nobody signed in), `secondFactor`, the session's SSO flow (`authFlowId`) and the
/// flows an account without a password may sign in again with (`authFlowList`). With `required`, a sign-in that is not
/// recent is also an error.
fn check_recent_auth(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let max_age = param_text(parameters, "maxAgeSeconds")
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|seconds| *seconds >= 0)
        .unwrap_or(DEFAULT_MAX_AGE_SECONDS);
    let mut out = BTreeMap::new();
    let user_id = crate::user::current_user_id()
        .map_err(|err| LibError::new(err.message()))?
        .filter(|id| !id.is_empty());
    let Some(user_id) = user_id else {
        out.insert("recent".to_string(), Value::Bool(false));
        out.insert("method".to_string(), Value::string("none"));
        if truthy(parameters, "required") {
            add_error(localized("Sign in first"));
        }
        return Ok(out);
    };
    let recent = crate::user::recent_auth(max_age.saturating_mul(1000));
    let age = crate::user::last_auth_millis().map(|at| (crate::user::now_millis() - at).max(0) / 1000);
    let account = facade
        .find("moqui.security.UserAccount")?
        .disable_authz()
        .eq("userId", Value::string(&user_id))
        .one()?;
    let has_password = account
        .as_ref()
        .is_some_and(|row| !field(row, "currentPassword").is_empty());
    let second_factor =
        has_password && crate::user::second_factor_required(&user_id).map_err(|err| LibError::new(err.message()))?;
    let flows: Vec<Value> = if has_password {
        Vec::new()
    } else {
        facade
            .find("moqui.security.sso.AuthFlow")?
            .disable_authz()
            .order_by("sequenceNum", false)
            .list()?
            .iter()
            .filter(|flow| field(flow, "disabled") != "Y" && field(flow, "inbound") != "Y")
            // only a flow whose round trip can answer the check: OpenID Connect, or an OAuth provider that asks the user
            // to sign in every time (SEC-WEB-84)
            .filter(|flow| match field(flow, "authFlowTypeEnumId").as_str() {
                "AftOidc" => true,
                "AftOauth" => facade
                    .find("moqui.security.sso.OauthFlow")
                    .and_then(|find| find.disable_authz().eq("authFlowId", Value::string(field(flow, "authFlowId"))).one())
                    .ok()
                    .flatten()
                    .is_some_and(|row| field(&row, "reauthPrompts") == "Y"),
                _ => false,
            })
            .map(|flow| {
                let id = field(flow, "authFlowId");
                let description = field(flow, "description");
                Value::Map(
                    [
                        ("authFlowId".to_string(), Value::string(&id)),
                        (
                            "description".to_string(),
                            Value::string(if description.is_empty() {
                                id.clone()
                            } else {
                                description
                            }),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                )
            })
            .collect()
    };
    out.insert("recent".to_string(), Value::Bool(recent));
    out.insert("authAgeSeconds".to_string(), age.map_or(Value::Null, Value::I64));
    out.insert(
        "method".to_string(),
        Value::string(if has_password { "password" } else { "sso" }),
    );
    out.insert("secondFactor".to_string(), Value::Bool(second_factor));
    out.insert(
        "authFlowId".to_string(),
        crate::user::session::session_attribute(SSO_FLOW_ID).map_or(Value::Null, Value::String),
    );
    out.insert("authFlowList".to_string(), Value::List(flows));
    if !recent && truthy(parameters, "required") {
        add_error(localized(REAUTH_TEXT));
    }
    Ok(out)
}

/// `reauthenticate#User`: `reauthenticated` when the password (and the code, when the account has a second factor) was
/// right; else `codeNeeded` with `codeText` and `sendableFactors` ({factorId, label}). A wrong password is an error.
fn reauthenticate_user(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    match crate::user::reauthenticate(
        &param_text(parameters, "password"),
        param_text(parameters, "code").trim(),
    ) {
        Ok(CodeLogin::LoggedIn) => {
            out.insert("reauthenticated".to_string(), Value::Bool(true));
            out.insert("codeNeeded".to_string(), Value::Bool(false));
        }
        Ok(CodeLogin::CodeNeeded { text, sendable }) => {
            out.insert("reauthenticated".to_string(), Value::Bool(false));
            out.insert("codeNeeded".to_string(), Value::Bool(true));
            out.insert("codeText".to_string(), Value::string(text));
            let factors = sendable
                .into_iter()
                .map(|factor| {
                    Value::Map(
                        [
                            ("factorId".to_string(), Value::string(factor.factor_id)),
                            ("label".to_string(), Value::string(factor.label)),
                        ]
                        .into_iter()
                        .collect(),
                    )
                })
                .collect();
            out.insert("sendableFactors".to_string(), Value::List(factors));
        }
        Err(err) => {
            out.insert("reauthenticated".to_string(), Value::Bool(false));
            add_error(localized(err.message()));
        }
    }
    Ok(out)
}

/// `send#ReauthCode`: a code to the factor `factorId`, after `reauthenticate#User` checked the password.
fn send_reauth_code(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let sent = match crate::user::send_reauth_code(&param_text(parameters, "factorId")) {
        Ok(()) => true,
        Err(err) => {
            add_error(localized(err.message()));
            false
        }
    };
    Ok([("sent".to_string(), Value::Bool(sent))].into_iter().collect())
}
