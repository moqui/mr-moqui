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

use std::collections::BTreeMap;

use moqui_lib::object::Value;

use super::{
    active_members, call, field, logout, map_fields, session_attribute, set_session_attribute, sync_account, ACCOUNT,
    FLOW_ID,
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
    crate::user::login_external_user("sso.out", None).unwrap();
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

/// The sign-in an identity provider proves: an OpenID Connect `auth_time` (after the request for a re-authentication,
/// none when the provider answered from an older session or sent none), an OAuth re-authentication taken at its word.
#[test]
fn an_sso_sign_in_counts_from_its_auth_time() {
    use super::sign_in_time;
    use crate::sso::{FlowConfig, FlowKind, OauthConfig, OidcConfig};
    let oidc = FlowConfig {
        auth_flow_id: "OIDC".into(),
        kind: FlowKind::Oidc(OidcConfig::default()),
    };
    let oauth = FlowConfig {
        auth_flow_id: "OAUTH".into(),
        kind: FlowKind::Oauth(OauthConfig::default()),
    };
    let now = 1_800_000_000_000_i64;
    let with_auth_time = |seconds: Option<i64>| {
        let mut profile = SsoProfile::default();
        if let Some(seconds) = seconds {
            profile.attributes.insert("auth_time".into(), Value::I64(seconds));
        }
        profile
    };
    let fresh = with_auth_time(Some(now / 1000 - 2));
    let old = with_auth_time(Some(now / 1000 - 3600));
    let none = with_auth_time(None);
    let started = Some(now - 10_000);
    // a plain login counts from when the provider says the user signed in; recent_auth then judges its age
    assert_eq!(sign_in_time(&oidc, &fresh, false, started, now), Some(now - 2000));
    assert_eq!(sign_in_time(&oidc, &old, false, started, now), Some(now - 3_600_000));
    assert_eq!(sign_in_time(&oidc, &none, false, started, now), None);
    // a re-authentication needs an auth_time after the request
    assert_eq!(sign_in_time(&oidc, &fresh, true, started, now), Some(now - 2000));
    assert_eq!(sign_in_time(&oidc, &old, true, started, now), None);
    assert_eq!(sign_in_time(&oidc, &none, true, started, now), None);
    assert_eq!(sign_in_time(&oidc, &fresh, true, None, now), None);
    // an auth_time ahead of this server's clock is now
    assert_eq!(
        sign_in_time(&oidc, &with_auth_time(Some(now / 1000 + 30)), false, started, now),
        Some(now)
    );
    // OAuth cannot prove it: only a re-authentication the user asked for counts
    assert_eq!(sign_in_time(&oauth, &none, true, started, now), Some(now));
    assert_eq!(sign_in_time(&oauth, &none, false, started, now), None);
}
