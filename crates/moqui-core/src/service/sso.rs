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

//! `org.moqui.sso.AuthServices`: `login#User`, `handle#Callback`, and `logout#User`, from the `moqui-sso` component's
//! `AuthenticationFlow` and `MoquiSecurityGrantedAccessAdapter`, on the host's [`crate::sso::SsoClient`].
//!
//! The account sync is Java's: the `AuthFlowFieldMap` rules map the profile attributes (an expression, or a mapping
//! service), the `UserAccount` is updated through `update#UserAccount` or created, and `UserGroupMember` follows
//! `AuthFlowRoleMap`: a mapped role adds its group, a current group no mapped role names is ended unless it is the
//! flow's `defaultUserGroupId`, and a user left with no group gets the default group. Unlike Java, the account is first
//! the one whose `externalUserId` is the provider's id, and only then the one with the profile's username; an account
//! linked to a different provider id is refused. The sync runs in its own transaction, so a failure part way changes
//! nothing. The login replaces the session id. A failure is logged and the browser goes back to `/Login`, as in Java.

use std::collections::{BTreeMap, BTreeSet};

use super::util::param_text;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use crate::context::ExecutionContext;
use crate::entity::{EntityFacade, EntityValue};
use crate::service::call::{sync_with, CallOptions};
use crate::sso::{FlowConfig, FlowKind, OauthConfig, OidcConfig, SsoProfile};
use crate::transaction::TransactionAttribute;
use crate::user::session::{session_attribute, set_session_attribute};

const ACCOUNT: &str = "moqui.security.UserAccount";

const PENDING: &str = "moquiAuthFlowPending";
const RETURN_TO: &str = "moquiAuthFlowReturnTo";
const EXTERNAL_LOGOUT: &str = "moquiAuthFlowExternalLogout";
const FLOW_ID: &str = "moquiAuthFlowId";
const ID_TOKEN: &str = "moquiAuthFlowIdToken";

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, login_user)?;
    rust_service!(map, handle_callback)?;
    rust_service!(map, logout_user)?;
    Ok(())
}

fn login_user(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    finish("org.moqui.sso.AuthServices.login#User", login(parameters, facade))
}

fn handle_callback(
    _parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    finish("org.moqui.sso.AuthServices.handle#Callback", callback(facade))
}

fn logout_user(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    finish("org.moqui.sso.AuthServices.logout#User", logout(parameters, facade))
}

/// A failed login, callback, or logout is logged and the browser goes back to `/Login`. The call still returns.
fn finish(name: &str, outcome: Result<(), LibError>) -> Result<BTreeMap<String, Value>, LibError> {
    if let Err(err) = outcome {
        tracing::error!("SSO {name} failed: {}", err.message());
        crate::web::send_redirect(&format!("{}/Login", base()));
    }
    Ok(BTreeMap::new())
}

/// The absolute root of this server, for the callback URL the provider redirects to: the render's base URL when it is
/// absolute, else the scheme and host of the request, else the conf `webroot` URL.
fn base() -> String {
    let base = crate::web::base_url();
    if base.contains("://") {
        return base.trim_end_matches('/').to_string();
    }
    if let Some(url) = crate::web::request_url() {
        if let Some((scheme, rest)) = url.split_once("://") {
            let host = rest.split('/').next().unwrap_or("");
            return format!("{scheme}://{host}");
        }
    }
    ExecutionContext::with_current(|ctx| ctx.factory().webapp_root_url("webroot"))
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string()
}

fn field(row: &EntityValue, name: &str) -> String {
    row.get(name)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .unwrap_or_default()
}

/// The same-origin `returnTo`, or none.
fn safe_return_to(url: &str) -> Option<String> {
    (!url.is_empty() && crate::web::is_same_origin_redirect(url, None)).then(|| url.to_string())
}

fn find_one(facade: &EntityFacade, entity: &str, id: &str) -> Result<Option<EntityValue>, LibError> {
    if id.is_empty() {
        return Ok(None);
    }
    facade
        .find(entity)?
        .disable_authz()
        .eq("authFlowId", Value::string(id))
        .one()
}

/// The flow and its protocol row. A disabled or inbound flow is not a login.
fn load_flow(facade: &EntityFacade, auth_flow_id: &str) -> Result<(EntityValue, FlowConfig), LibError> {
    let flow = find_one(facade, "moqui.security.sso.AuthFlow", auth_flow_id)?
        .ok_or_else(|| LibError::new(format!("AuthFlow {auth_flow_id} not found")))?;
    if field(&flow, "disabled") == "Y" || field(&flow, "inbound") == "Y" {
        return Err(LibError::new(format!(
            "AuthFlow {auth_flow_id} is not available for login"
        )));
    }
    let kind = match field(&flow, "authFlowTypeEnumId").as_str() {
        "AftOidc" => {
            let row = find_one(facade, "moqui.security.sso.OidcFlow", auth_flow_id)?
                .ok_or_else(|| LibError::new(format!("AuthFlow {auth_flow_id} has no OidcFlow")))?;
            let algorithm_id = field(&row, "preferredJwsAlgorithmEnumId");
            let preferred_jws_algorithm = if algorithm_id.is_empty() {
                String::new()
            } else {
                facade
                    .find("moqui.basic.Enumeration")?
                    .disable_authz()
                    .eq("enumId", Value::string(&algorithm_id))
                    .one()?
                    .map(|row| field(&row, "optionValue"))
                    .unwrap_or_default()
            };
            FlowKind::Oidc(OidcConfig {
                client_type: field(&row, "clientTypeEnumId"),
                client_id: field(&row, "clientId"),
                secret: field(&row, "secret"),
                discovery_uri: field(&row, "discoveryUri"),
                realm: field(&row, "realm"),
                base_uri: field(&row, "baseUri"),
                preferred_jws_algorithm,
                use_nonce: field(&row, "useNonce") == "Y",
                apple_team_id: field(&row, "appleTeamId"),
                apple_key_id: field(&row, "appleKeyId"),
            })
        }
        "AftOauth" => {
            let row = find_one(facade, "moqui.security.sso.OauthFlow", auth_flow_id)?
                .ok_or_else(|| LibError::new(format!("AuthFlow {auth_flow_id} has no OauthFlow")))?;
            FlowKind::Oauth(OauthConfig {
                client_type: field(&row, "clientTypeEnumId"),
                client_id: field(&row, "clientId"),
                secret: field(&row, "secret"),
                authorize_url: field(&row, "authorizeUrl"),
                token_url: field(&row, "tokenUrl"),
                user_info_url: field(&row, "userInfoUrl"),
                scope: field(&row, "scope"),
            })
        }
        "AftSaml" => {
            return Err(LibError::new(format!(
                "AuthFlow {auth_flow_id} is SAML, which is not available in this port"
            )))
        }
        other => {
            return Err(LibError::new(format!(
                "AuthFlow {auth_flow_id} has unknown type {other}"
            )))
        }
    };
    Ok((
        flow,
        FlowConfig {
            auth_flow_id: auth_flow_id.to_string(),
            kind,
        },
    ))
}

/// `login#User`: keep a same-origin `returnTo`, start the flow, keep its state in the session, and redirect.
fn login(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Result<(), LibError> {
    let auth_flow_id = param_text(parameters, "authFlowId");
    let (_, flow) = load_flow(facade, &auth_flow_id)?;
    // Without a returnTo the one an earlier, unfinished login stored is cleared; Java keeps it for this login.
    match safe_return_to(&param_text(parameters, "returnTo")) {
        Some(url) => set_session_attribute(RETURN_TO, Some(&url)),
        None => set_session_attribute(RETURN_TO, None),
    }
    let redirect_uri = format!("{}/sso/callback", base());
    let started = crate::sso::client()?.start(&flow, &redirect_uri)?;
    let pending = serde_json::json!({ "authFlowId": auth_flow_id, "pending": started.pending });
    set_session_attribute(PENDING, Some(&pending.to_string()));
    crate::web::send_redirect(&started.authorize_url);
    Ok(())
}

/// `handle#Callback`: complete the flow the session started, sync the account, log in.
fn callback(facade: &EntityFacade) -> Result<(), LibError> {
    let stored = session_attribute(PENDING).ok_or_else(|| LibError::new("no SSO login is in progress"))?;
    // One callback per login: the state cannot be replayed.
    set_session_attribute(PENDING, None);
    let stored: serde_json::Value =
        serde_json::from_str(&stored).map_err(|err| LibError::new(format!("SSO state is not readable: {err}")))?;
    let auth_flow_id = stored
        .get("authFlowId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let pending = stored
        .get("pending")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let (flow_row, flow) = load_flow(facade, &auth_flow_id)?;
    // The OIDC and OAuth parameters only. `RelayState`, which Java takes as the return URL ahead of the session's, is
    // SAML's (not in this port), so a return URL is never taken from the callback's query.
    let callback: BTreeMap<String, String> = crate::web::parameters()
        .into_iter()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "code" | "state" | "iss" | "error" | "error_description" | "error_uri"
            )
        })
        .map(|(name, value)| (name, to_plain_string(&value)))
        .collect();
    let profile = crate::sso::client()?.complete(&flow, &pending, &callback)?;
    let username = profile
        .username
        .clone()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| LibError::new(format!("AuthFlow {auth_flow_id}: the provider gave no username")))?;
    let username = sync_account(facade, &flow_row, &profile, &username)?;
    crate::user::login_external_user(&username).map_err(|err| LibError::new(err.message()))?;
    set_session_attribute(EXTERNAL_LOGOUT, Some("true"));
    set_session_attribute(FLOW_ID, Some(&auth_flow_id));
    set_session_attribute(ID_TOKEN, profile.id_token.as_deref());
    match session_attribute(RETURN_TO).and_then(|url| safe_return_to(&url)) {
        Some(url) => set_session_attribute(RETURN_TO, Some(&url)),
        None => set_session_attribute(RETURN_TO, None),
    }
    tracing::info!("SSO login of {username} through AuthFlow {auth_flow_id}");
    Ok(())
}

/// `logout#User`: end the local login, then send the browser to the provider's end-session URL when it has one,
/// else to a same-origin `returnTo` or `/Login`. The local login ends even when the provider's URL cannot be had,
/// where Java leaves the user logged in when its pac4j logout throws.
fn logout(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Result<(), LibError> {
    let base = base();
    let after = safe_return_to(&param_text(parameters, "returnTo")).unwrap_or_else(|| format!("{base}/Login"));
    let after = if after.starts_with('/') {
        format!("{base}{after}")
    } else {
        after
    };
    let auth_flow_id = session_attribute(FLOW_ID).unwrap_or_default();
    let id_token = session_attribute(ID_TOKEN);
    let provider_url = if auth_flow_id.is_empty() {
        None
    } else {
        let url = load_flow(facade, &auth_flow_id)
            .and_then(|(_, flow)| crate::sso::client()?.logout_url(&flow, id_token.as_deref(), &after));
        match url {
            Ok(url) => url,
            Err(err) => {
                tracing::warn!("SSO logout: {}", err.message());
                None
            }
        }
    };
    for name in [EXTERNAL_LOGOUT, FLOW_ID, ID_TOKEN, RETURN_TO] {
        set_session_attribute(name, None);
    }
    crate::user::logout().map_err(|err| LibError::new(err.message()))?;
    crate::web::send_redirect(provider_url.as_deref().unwrap_or(&after));
    Ok(())
}

/// A service call with authorization off, as the anonymous-all callback runs it. A failure is an error here.
fn call(name: &str, parameters: BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, LibError> {
    let options = CallOptions {
        disable_authz: true,
        ..CallOptions::default()
    };
    let mark = ExecutionContext::with_current(|ctx| ctx.message().error_mark()).unwrap_or_default();
    let result = sync_with(name, &parameters, &options);
    let errors =
        ExecutionContext::with_current(|ctx| ctx.message().take_errors_since(mark).join("\n")).unwrap_or_default();
    match result {
        Ok(_) if !errors.is_empty() => Err(LibError::new(format!("{name}: {errors}"))),
        Ok(out) => Ok(out),
        Err(err) => Err(LibError::new(format!("{name}: {}", err.message()))),
    }
}

/// `mapProfileFields`: each rule's expression (or its source field name) against the profile attributes; a value goes
/// to `dstFieldName`, or through the rule's mapping service, whose `destinationMap` and `sourceMap` are merged.
fn map_fields(
    facade: &EntityFacade,
    auth_flow_id: &str,
    attributes: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, LibError> {
    let rules = facade
        .find("moqui.security.sso.AuthFlowFieldMap")?
        .disable_authz()
        .eq("authFlowId", Value::string(auth_flow_id))
        .order_by("ruleSeqId", false)
        .list()?;
    let mut out = BTreeMap::new();
    for rule in rules.values() {
        let expression = match field(rule, "dstFieldExpression") {
            text if text.is_empty() => field(rule, "srcFieldName"),
            text => text,
        };
        if expression.is_empty() {
            continue;
        }
        let value = {
            let _frame = crate::script::ContextFrame::push();
            for (name, value) in attributes {
                crate::script::set_context_name(name, value.clone()).map_err(|err| LibError::new(err.message()))?;
            }
            // A claim the provider did not send is no value, as a missing name in a Groovy binding.
            match crate::script::eval_value(&expression) {
                Ok(value) => value,
                Err(err) => {
                    tracing::debug!("AuthFlowFieldMap {expression}: {}", err.message());
                    Value::Null
                }
            }
        };
        // Groovy truth, as `if (srcFieldValue)`: false and a zero number are no value too.
        if is_empty(&value) || !groovy_truth(&value) {
            continue;
        }
        let destination = field(rule, "dstFieldName");
        let register_id = field(rule, "mappingServiceRegisterId");
        if register_id.is_empty() {
            out.insert(destination, value);
            continue;
        }
        let register = facade
            .find("moqui.service.ServiceRegister")?
            .disable_authz()
            .eq("serviceRegisterId", Value::string(&register_id))
            .one()?
            .ok_or_else(|| LibError::new(format!("ServiceRegister {register_id} not found")))?;
        let mut parameters = BTreeMap::new();
        parameters.insert("srcFieldValue".to_string(), value);
        parameters.insert("dstFieldName".to_string(), Value::string(destination));
        let mapped = call(&field(&register, "serviceName"), parameters)?;
        for key in ["destinationMap", "sourceMap"] {
            if let Some(Value::Map(map)) = mapped.get(key) {
                out.extend(map.iter().map(|(name, value)| (name.clone(), value.clone())));
            }
        }
    }
    Ok(out)
}

/// False for `false` and a zero number; the empty values are `is_empty`.
fn groovy_truth(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::I32(number) => *number != 0,
        Value::I64(number) => *number != 0,
        Value::F64(number) => *number != 0.0,
        Value::Decimal(number) => !number.is_zero(),
        _ => true,
    }
}

fn now_value() -> Value {
    crate::user::now_value()
}

/// `MoquiSecurityGrantedAccessAdapter.adapt` for one profile, in its own transaction, so a failure part way leaves the
/// account and its groups as they were. Returns the username to log in as.
fn sync_account(
    facade: &EntityFacade,
    flow: &EntityValue,
    profile: &SsoProfile,
    username: &str,
) -> Result<String, LibError> {
    let tx = facade.transactions_for(ACCOUNT)?;
    tx.run(TransactionAttribute::ForceNew, None, || {
        sync_account_rows(facade, flow, profile, username)
    })
}

fn sync_account_rows(
    facade: &EntityFacade,
    flow: &EntityValue,
    profile: &SsoProfile,
    username: &str,
) -> Result<String, LibError> {
    let auth_flow_id = field(flow, "authFlowId");
    let mapped = map_fields(facade, &auth_flow_id, &profile.attributes)?;
    let find_by = |name: &str, value: &str| -> Result<Option<EntityValue>, LibError> {
        facade
            .find(ACCOUNT)?
            .disable_authz()
            .eq(name, Value::string(value))
            .use_cache(false)
            .one()
    };
    let linked = if profile.id.is_empty() {
        None
    } else {
        find_by("externalUserId", &profile.id)?
    };
    let existing = match linked {
        Some(account) => Some(account),
        None => find_by("username", username)?,
    };
    let mut parameters = mapped;
    parameters.insert("externalUserId".to_string(), Value::string(&profile.id));
    let (user_id, username) = match existing {
        Some(account) => {
            let linked_id = field(&account, "externalUserId");
            if !linked_id.is_empty() && linked_id != profile.id {
                return Err(LibError::new(format!(
                    "AuthFlow {auth_flow_id}: the account {username} is linked to another user of the provider"
                )));
            }
            let user_id = field(&account, "userId");
            let username = field(&account, "username");
            parameters.insert("userId".to_string(), Value::string(&user_id));
            parameters.insert("username".to_string(), Value::string(&username));
            call("org.moqui.impl.UserServices.update#UserAccount", parameters)?;
            (user_id, username)
        }
        None => {
            parameters.insert("username".to_string(), Value::string(username));
            let created = call("create#moqui.security.UserAccount", parameters)?;
            let user_id = created.get("userId").map(to_plain_string).unwrap_or_default();
            (user_id, username.to_string())
        }
    };
    if user_id.is_empty() {
        return Err(LibError::new(format!("no UserAccount for {username}")));
    }

    let now = now_value();
    let current = active_members(facade, &user_id)?;
    let mut obsolete: BTreeSet<String> = current.iter().map(|member| field(member, "userGroupId")).collect();
    let roles = profile_roles(profile);
    for role in roles.iter().map(String::as_str) {
        let mapped = facade
            .find("moqui.security.sso.AuthFlowRoleMap")?
            .disable_authz()
            .eq("authFlowId", Value::string(&auth_flow_id))
            .eq("roleName", Value::string(role))
            .one()?;
        let Some(group) = mapped.map(|row| field(&row, "userGroupId")).filter(|id| !id.is_empty()) else {
            continue;
        };
        if !obsolete.remove(&group) {
            add_member(&group, &user_id, &now)?;
        }
    }
    let default_group = field(flow, "defaultUserGroupId");
    for member in &current {
        let group = field(member, "userGroupId");
        if obsolete.contains(&group) && group != default_group {
            let mut parameters = BTreeMap::new();
            parameters.insert("userGroupId".to_string(), Value::string(&group));
            parameters.insert("userId".to_string(), Value::string(&user_id));
            parameters.insert(
                "fromDate".to_string(),
                member.get("fromDate").cloned().unwrap_or(Value::Null),
            );
            parameters.insert("thruDate".to_string(), now.clone());
            call("update#moqui.security.UserGroupMember", parameters)?;
        }
    }
    if !default_group.is_empty() && active_members(facade, &user_id)?.is_empty() {
        add_member(&default_group, &user_id, &now)?;
    }
    Ok(username)
}

fn add_member(group: &str, user_id: &str, now: &Value) -> Result<(), LibError> {
    let mut parameters = BTreeMap::new();
    parameters.insert("userGroupId".to_string(), Value::string(group));
    parameters.insert("userId".to_string(), Value::string(user_id));
    parameters.insert("fromDate".to_string(), now.clone());
    call("create#moqui.security.UserGroupMember", parameters).map(|_| ())
}

/// The user's `UserGroupMember` rows in date now.
fn active_members(facade: &EntityFacade, user_id: &str) -> Result<Vec<EntityValue>, LibError> {
    let now = crate::user::now_millis();
    let rows = facade
        .find("moqui.security.UserGroupMember")?
        .disable_authz()
        .eq("userId", Value::string(user_id))
        .use_cache(false)
        .list()?;
    Ok(rows
        .values()
        .iter()
        .filter(|row| {
            let from = row.get("fromDate").and_then(crate::user::value_millis);
            let thru = row.get("thruDate").and_then(crate::user::value_millis);
            from.is_none_or(|from| from <= now) && thru.is_none_or(|thru| thru > now)
        })
        .cloned()
        .collect())
}

/// The provider's roles; when it gave none apart, the `roles` claim among the attributes (a list, or text split on
/// commas), as the Java client reads a profile's roles.
fn profile_roles(profile: &SsoProfile) -> BTreeSet<String> {
    if !profile.roles.is_empty() {
        return profile.roles.iter().cloned().collect();
    }
    match profile.attributes.get("roles") {
        Some(Value::List(items)) => items
            .iter()
            .map(to_plain_string)
            .filter(|role| !role.is_empty())
            .collect(),
        Some(Value::String(text)) => text
            .split(',')
            .map(str::trim)
            .filter(|role| !role.is_empty())
            .map(str::to_string)
            .collect(),
        _ => BTreeSet::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use moqui_lib::object::Value;

    use super::{
        active_members, call, field, logout, map_fields, session_attribute, set_session_attribute, sync_account,
        ACCOUNT, FLOW_ID,
    };
    use crate::context::ExecutionContextFactory;
    use crate::entity::{EntityFacade, EntityValue};
    use crate::service::eca_rules;
    use crate::sso::SsoProfile;

    fn entered() -> (EntityFacade, crate::context::ExecutionContext) {
        let facade = crate::testing::open();
        let context = ExecutionContextFactory::test_context_in(
            "",
            crate::service::ServicePools::idle(),
            crate::context::framework_dir(),
        );
        context.factory().set_entity(facade.clone());
        (facade, context)
    }

    fn seed(facade: &EntityFacade, entity: &str, fields: &[(&str, Value)]) -> EntityValue {
        let mut row = facade.make_value(entity).expect("make");
        for (name, value) in fields {
            row.set(*name, value.clone());
        }
        row.create(facade).expect("create");
        row
    }

    #[test]
    fn roles_fall_back_to_the_roles_claim() {
        let mut given = profile("u", "u", &["a"]);
        given.attributes.insert("roles".to_string(), Value::string("x, y"));
        assert_eq!(super::profile_roles(&given).into_iter().collect::<Vec<_>>(), ["a"]);
        given.roles.clear();
        assert_eq!(super::profile_roles(&given).into_iter().collect::<Vec<_>>(), ["x", "y"]);
        given
            .attributes
            .insert("roles".to_string(), Value::List(vec![Value::string("z")]));
        assert_eq!(super::profile_roles(&given).into_iter().collect::<Vec<_>>(), ["z"]);
    }

    fn profile(id: &str, username: &str, roles: &[&str]) -> SsoProfile {
        let mut attributes = BTreeMap::new();
        attributes.insert("name".to_string(), Value::string("New Name"));
        SsoProfile {
            id: id.to_string(),
            username: Some(username.to_string()),
            attributes,
            roles: roles.iter().map(|role| (*role).to_string()).collect(),
            id_token: None,
        }
    }

    fn account(facade: &EntityFacade, user_id: &str) -> EntityValue {
        facade
            .find(ACCOUNT)
            .unwrap()
            .disable_authz()
            .eq("userId", Value::string(user_id))
            .use_cache(false)
            .one()
            .unwrap()
            .unwrap()
    }

    /// A flow mapping `name` to `userFullName`, and roles `a` and `b` to the groups `NEW_GROUP` and `BROKEN`.
    fn flow(facade: &EntityFacade) -> EntityValue {
        let flow = seed(
            facade,
            "moqui.security.sso.AuthFlow",
            &[("authFlowId", Value::string("TEST_FLOW"))],
        );
        seed(
            facade,
            "moqui.security.sso.AuthFlowFieldMap",
            &[
                ("authFlowId", Value::string("TEST_FLOW")),
                ("ruleSeqId", Value::string("01")),
                ("srcFieldName", Value::string("name")),
                ("dstFieldName", Value::string("userFullName")),
            ],
        );
        for (role, group) in [("a", "NEW_GROUP"), ("b", "BROKEN")] {
            seed(
                facade,
                "moqui.security.sso.AuthFlowRoleMap",
                &[
                    ("authFlowId", Value::string("TEST_FLOW")),
                    ("roleName", Value::string(role)),
                    ("userGroupId", Value::string(group)),
                ],
            );
        }
        flow
    }

    #[test]
    fn a_failure_part_way_leaves_the_account_and_its_groups_as_they_were() {
        let (facade, context) = entered();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        crate::service::load_definitions().unwrap();
        eca_rules::install(
            context.factory(),
            eca_rules::from_xml(
                r#"<secas>
                    <seca service="create#moqui.security.UserGroupMember" when="post-service">
                        <condition><expression>userGroupId == 'BROKEN'</expression></condition>
                        <actions><script>ec.message.addError('group broke')</script></actions>
                    </seca>
                </secas>"#,
                "",
            ),
        );
        let flow = flow(&facade);
        seed(
            &facade,
            ACCOUNT,
            &[
                ("userId", Value::string("SSO_ANN")),
                ("username", Value::string("sso.ann")),
                ("userFullName", Value::string("Old Name")),
            ],
        );
        seed(
            &facade,
            "moqui.security.UserGroupMember",
            &[
                ("userGroupId", Value::string("OLD_GROUP")),
                ("userId", Value::string("SSO_ANN")),
                ("fromDate", crate::user::now_value()),
            ],
        );
        let err = sync_account(&facade, &flow, &profile("sub-ann", "sso.ann", &["a", "b"]), "sso.ann").unwrap_err();
        assert!(err.message().contains("group broke"), "{}", err.message());
        // The account update and the NEW_GROUP membership made before the failure rolled back.
        let groups: Vec<String> = active_members(&facade, "SSO_ANN")
            .unwrap()
            .iter()
            .map(|member| field(member, "userGroupId"))
            .collect();
        assert_eq!(groups, ["OLD_GROUP"]);
        let account = account(&facade, "SSO_ANN");
        assert_eq!(field(&account, "userFullName"), "Old Name");
        assert_eq!(field(&account, "externalUserId"), "");
        assert!(
            context.message().errors().is_empty(),
            "{:?}",
            context.message().errors()
        );
    }

    #[test]
    fn a_username_linked_to_another_provider_id_is_refused_and_a_linked_id_wins() {
        let (facade, context) = entered();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        crate::service::load_definitions().unwrap();
        let flow = flow(&facade);
        seed(
            &facade,
            ACCOUNT,
            &[
                ("userId", Value::string("SSO_ADMIN")),
                ("username", Value::string("admin")),
                ("externalUserId", Value::string("sub-admin")),
                ("userFullName", Value::string("Admin")),
            ],
        );
        let err = sync_account(&facade, &flow, &profile("sub-other", "admin", &[]), "admin").unwrap_err();
        assert!(err.message().contains("linked to another user"), "{}", err.message());
        let admin = account(&facade, "SSO_ADMIN");
        assert_eq!(field(&admin, "externalUserId"), "sub-admin");
        assert_eq!(field(&admin, "userFullName"), "Admin");

        // The account already linked to the provider's id is the one logged in, whatever username the provider sends.
        let username = sync_account(&facade, &flow, &profile("sub-admin", "renamed", &[]), "renamed").unwrap();
        assert_eq!(username, "admin");
        assert_eq!(field(&account(&facade, "SSO_ADMIN"), "userFullName"), "New Name");
    }

    #[test]
    fn a_zero_or_false_claim_is_not_mapped_as_groovy_truth_has_it() {
        let (facade, context) = entered();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        crate::service::load_definitions().unwrap();
        // The flow maps the `name` claim to `userFullName`.
        flow(&facade);
        let mapped = |value: Value| {
            let mut attributes = BTreeMap::new();
            attributes.insert("name".to_string(), value);
            map_fields(&facade, "TEST_FLOW", &attributes)
                .unwrap()
                .get("userFullName")
                .map(moqui_lib::object::to_plain_string)
        };
        for value in [
            Value::I64(0),
            Value::I32(0),
            Value::F64(0.0),
            Value::Decimal(moqui_lib::format::Decimal::ZERO),
            Value::Bool(false),
            Value::string(""),
        ] {
            assert_eq!(mapped(value.clone()), None, "{value:?}");
        }
        assert_eq!(mapped(Value::I64(7)).as_deref(), Some("7"));
        assert_eq!(mapped(Value::string("0")).as_deref(), Some("0"));
    }

    #[test]
    fn logout_ends_the_local_login_when_the_provider_url_cannot_be_had() {
        let (facade, context) = entered();
        let _entered = context.enter();
        crate::service::load_definitions().unwrap();
        {
            let _authz = context.disable_authz();
            seed(
                &facade,
                "moqui.security.sso.AuthFlow",
                &[
                    ("authFlowId", Value::string("OIDC_FLOW")),
                    ("authFlowTypeEnumId", Value::string("AftOidc")),
                ],
            );
            seed(
                &facade,
                "moqui.security.sso.OidcFlow",
                &[("authFlowId", Value::string("OIDC_FLOW"))],
            );
            seed(
                &facade,
                ACCOUNT,
                &[
                    ("userId", Value::string("SSO_OUT")),
                    ("username", Value::string("sso.out")),
                ],
            );
        }
        crate::user::login_external_user("sso.out").unwrap();
        assert_eq!(
            crate::user::session::current_user_id().unwrap().as_deref(),
            Some("SSO_OUT")
        );
        set_session_attribute(FLOW_ID, Some("OIDC_FLOW"));
        // No SSO client is installed in this crate's tests, so the provider's end-session URL is an error.
        assert!(crate::sso::client().is_err());
        logout(&BTreeMap::new(), &facade).unwrap();
        assert_eq!(crate::user::session::current_user_id().unwrap(), None);
        assert_eq!(session_attribute(FLOW_ID), None);
    }

    #[test]
    fn a_call_takes_only_the_errors_it_added() {
        let (_facade, context) = entered();
        let _entered = context.enter();
        crate::service::load_definitions().unwrap();
        context.message().add_message("kept");
        let mut parameters = BTreeMap::new();
        parameters.insert("userId".to_string(), Value::string("NOBODY"));
        let err = call("org.moqui.impl.UserServices.update#UserAccount", parameters.clone()).unwrap_err();
        assert!(err.message().contains("not found"), "{}", err.message());
        assert!(context.message().errors().is_empty());
        assert_eq!(context.message().messages(), ["kept"]);
        // An earlier error stops the service, as in any call, and is not cleared.
        context.message().add_error("from before");
        call("org.moqui.impl.UserServices.update#UserAccount", parameters).unwrap();
        assert_eq!(context.message().errors(), ["from before"]);
    }
}
