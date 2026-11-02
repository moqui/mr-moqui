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

//! Delegated calls (plans/Edge-Node.md §5.2). On an edge node the service facade hands a synchronous call of a
//! service the conf lists (`<edge><delegate service=".."/>`) to the link (`crate::service::call::sync_with`, which
//! checks authorization first when the edge node has the service's definition): the parameters and the caller's
//! identity go as JSON data, and the local node's result or errors come back. On the local node [`run`] runs it as the user `_EDGE_`, with authorization, and only
//! when its own conf lists the service too.

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_lib::json::{from_json, to_json};
use moqui_lib::object::Value;

use super::{DelegateAnswer, DelegateCall, OFFLINE};
use crate::context::{Error, ExecutionContext, ExecutionContextFactory};

/// The user a local node runs delegated calls and queued items as (framework/data/EdgeSetupData.xml).
pub const EDGE_USERNAME: &str = "_edge_";

/// The parameter a delegated service declares to read who called it on the edge node: a map of `username` and
/// `userId`.
pub const CALLER_PARAMETER: &str = "edgeCaller";

/// Parameters that never cross the link, and that the local node drops if they do: a login's credentials, and the
/// caller, which the local node sets from the call's caller data. Any other `auth` name ([`is_auth_name`]) and any
/// `_` name are dropped too.
const NOT_SENT: [&str; 5] = [
    "authUsername",
    "authPassword",
    "authUserAccount",
    "authTenantId",
    CALLER_PARAMETER,
];

/// Whether `name` is a login parameter of the service facade's kind: `auth` then an upper case letter
/// (`authUsername`, `authUserAccount`), or `auth` alone. `author` is not.
fn is_auth_name(name: &str) -> bool {
    name.strip_prefix("auth")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(|ch: char| ch.is_ascii_uppercase()))
}

/// Whether a parameter may cross the link: never credentials, a caller, or the service facade's own `_` names.
fn crosses(name: &str) -> bool {
    !NOT_SENT.contains(&name) && !is_auth_name(name) && !name.starts_with('_')
}

/// On an edge node, the conf entry of a call of `name` (canonical) that is to run on the local node; `None` for any
/// other call, which runs here as usual. Only synchronous calls are delegated: an async call or a scheduled job of a
/// listed service runs on the node that queued it.
pub(crate) fn listed(name: &str) -> Option<super::conf::Delegate> {
    if !super::conf::delegating() {
        return None;
    }
    let factory = ExecutionContext::with_current(|ctx| Arc::clone(ctx.factory()))?;
    let settings = super::settings(&factory);
    if !settings.edge_node {
        return None;
    }
    settings.delegate(name).cloned()
}

/// Send a delegated call of `name` and wait for the local node's answer: its result, or its errors added to this
/// context's messages. The caller's transaction, if any, stays open while it waits (at most the link's answer
/// timeout, 60 s).
pub(crate) fn send(
    name: &str,
    parameters: &BTreeMap<String, Value>,
    timeout: Option<std::time::Duration>,
) -> Result<BTreeMap<String, Value>, Error> {
    let call = DelegateCall {
        service: name.to_string(),
        parameters: parameters_json(parameters),
        caller: caller_json(),
        operation_id: crate::entity::uuid_v4().map_err(Error::from)?,
        timeout,
    };
    let link = super::link().map_err(|_| Error::new(format!("{OFFLINE}: this host has no edge link")))?;
    let answer = link.delegate(&call).map_err(Error::from)?;
    if !answer.errors.is_empty() {
        ExecutionContext::with_current(|ctx| {
            let mut messages = ctx.message();
            for error in &answer.errors {
                messages.add_error(error.clone());
            }
        });
    }
    // with its errors the result is what the service returned beside them (a refusal's reasonCode), as on this node
    Ok(result_map(&answer.result))
}

/// The parameters as a JSON object, without credentials, a caller, or `_` names.
pub fn parameters_json(parameters: &BTreeMap<String, Value>) -> serde_json::Value {
    let object: serde_json::Map<String, serde_json::Value> = parameters
        .iter()
        .filter(|(name, _)| crosses(name))
        .map(|(name, value)| (name.clone(), to_json(value)))
        .collect();
    serde_json::Value::Object(object)
}

/// On the local node, the parameters of a delegated call as the service gets them: whatever the edge node sent, less
/// any credentials, caller or `_` name (so nothing it sends can change the user the call runs as), plus the caller
/// as data.
pub(crate) fn received_parameters(call: &DelegateCall) -> BTreeMap<String, Value> {
    let mut parameters = result_map(&call.parameters);
    parameters.retain(|name, _| crosses(name));
    parameters.insert(CALLER_PARAMETER.to_string(), from_json(&call.caller));
    parameters
}

/// The current user as data: `username` and `userId`, null when no one is logged in.
fn caller_json() -> serde_json::Value {
    let username = crate::user::current_username().ok().flatten();
    let user_id = crate::user::session::current_user_id().ok().flatten();
    serde_json::json!({"username": username, "userId": user_id})
}

/// A JSON object as a service's result map; anything else is no result.
pub fn result_map(json: &serde_json::Value) -> BTreeMap<String, Value> {
    match from_json(json) {
        Value::Map(entries) => entries.into_iter().collect(),
        _ => BTreeMap::new(),
    }
}

/// On a local node, run a delegated call as `_EDGE_` on this thread (a request worker with a fresh context): refused
/// unless this node's conf lists the service, and run with authorization, so `_EDGE_`'s grants must cover it too.
pub fn run(factory: &ExecutionContextFactory, call: &DelegateCall) -> DelegateAnswer {
    let refused = |message: String| DelegateAnswer {
        result: serde_json::Value::Null,
        errors: vec![message],
    };
    let settings = super::settings(factory);
    if settings.edge_node {
        return refused("this node is an edge node; it runs no delegated calls".to_string());
    }
    let service = crate::service::call::canonical(&call.service);
    if !settings.delegates(&service) {
        tracing::warn!(
            "Edge link: refused a delegated call of {}, which this node's edge conf does not list (operation {})",
            call.service,
            call.operation_id
        );
        return refused(format!("{} is not a delegated service on this node", call.service));
    }
    if let Err(err) = crate::user::internal_login_user(EDGE_USERNAME) {
        tracing::error!("Edge link: the delegate user {EDGE_USERNAME} could not log in: {err}");
        return refused("this node's delegate user is not available".to_string());
    }
    let parameters = received_parameters(call);
    tracing::debug!(
        "Edge link: delegated call of {} (operation {})",
        call.service,
        call.operation_id
    );
    match crate::service::sync(&service, &parameters) {
        Ok(result) => {
            let errors = ExecutionContext::with_current(|ctx| ctx.message().errors().to_vec()).unwrap_or_default();
            // the result goes with the errors too: a refusal names its reasonCode beside its error (E8), which a caller
            // such as the SIP engine tells apart
            let object: serde_json::Map<String, serde_json::Value> = result
                .iter()
                .map(|(name, value)| (name.clone(), to_json(value)))
                .collect();
            DelegateAnswer {
                result: serde_json::Value::Object(object),
                errors,
            }
        }
        Err(err) => refused(err.message().to_string()),
    }
}

#[cfg(test)]
#[path = "delegate_tests.rs"]
mod tests;
