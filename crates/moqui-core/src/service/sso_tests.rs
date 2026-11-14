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
    // the provider's verified address, which the flow's rule links an existing account by
    attributes.insert("email".to_string(), Value::string(format!("{username}@example.com")));
    attributes.insert("email_verified".to_string(), Value::Bool(true));
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

/// A flow mapping `name` to `userFullName`, and roles `a` and `b` to the groups `NEW_GROUP` and `BROKEN`, that links an
/// existing account by the provider's verified email address.
fn flow(facade: &EntityFacade) -> EntityValue {
    let flow = seed(
        facade,
        "moqui.security.sso.AuthFlow",
        &[
            ("authFlowId", Value::string("TEST_FLOW")),
            ("accountLinkEnumId", Value::string("AflVerifiedEmail")),
        ],
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
            ("emailAddress", Value::string("SSO.Ann@example.com")),
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
        deny_internal: false,
    };
    let oauth = FlowConfig {
        auth_flow_id: "OAUTH".into(),
        kind: FlowKind::Oauth(OauthConfig::default()),
        deny_internal: false,
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

fn groups_of(facade: &EntityFacade, user_id: &str) -> Vec<String> {
    let mut groups: Vec<String> = active_members(facade, user_id)
        .unwrap()
        .iter()
        .map(|member| field(member, "userGroupId"))
        .collect();
    groups.sort();
    groups
}

#[test]
fn an_existing_account_is_linked_only_by_a_verified_email_and_never_a_privileged_one() {
    let (facade, context) = entered();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    crate::service::load_definitions().unwrap();
    let flow = flow(&facade);
    let person = |user_id: &str, username: &str, email: &str| {
        seed(
            &facade,
            ACCOUNT,
            &[
                ("userId", Value::string(user_id)),
                ("username", Value::string(username)),
                ("emailAddress", Value::string(email)),
            ],
        );
    };
    let member = |group: &str, user_id: &str| {
        seed(
            &facade,
            "moqui.security.UserGroupMember",
            &[
                ("userGroupId", Value::string(group)),
                ("userId", Value::string(user_id)),
                ("fromDate", crate::user::now_value()),
            ],
        );
    };
    // The provider's username alone links nothing: another address, an unverified one, and a flow with no rule.
    person("SSO_BEA", "sso.bea", "bea@elsewhere.example");
    let err = sync_account(&facade, &flow, &profile("sub-bea", "sso.bea", &[]), "sso.bea").unwrap_err();
    assert!(err.message().contains("no verified email"), "{}", err.message());
    person("SSO_CAL", "sso.cal", "sso.cal@example.com");
    let mut unverified = profile("sub-cal", "sso.cal", &[]);
    unverified
        .attributes
        .insert("email_verified".to_string(), Value::Bool(false));
    let err = sync_account(&facade, &flow, &unverified, "sso.cal").unwrap_err();
    assert!(err.message().contains("no verified email"), "{}", err.message());
    let no_rule = seed(
        &facade,
        "moqui.security.sso.AuthFlow",
        &[("authFlowId", Value::string("NO_RULE_FLOW"))],
    );
    let err = sync_account(&facade, &no_rule, &profile("sub-cal", "sso.cal", &[]), "sso.cal").unwrap_err();
    assert!(err.message().contains("links no existing account"), "{}", err.message());
    assert_eq!(field(&account(&facade, "SSO_CAL"), "externalUserId"), "");
    // The verified address links it; an instance administrator's account is linked by an administrator only.
    sync_account(&facade, &flow, &profile("sub-cal", "sso.cal", &[]), "sso.cal").unwrap();
    assert_eq!(field(&account(&facade, "SSO_CAL"), "externalUserId"), "sub-cal");
    person("SSO_ROOT", "sso.root", "sso.root@example.com");
    member("ADMIN", "SSO_ROOT");
    let err = sync_account(&facade, &flow, &profile("sub-root", "sso.root", &[]), "sso.root").unwrap_err();
    assert!(err.message().contains("an administrator links it"), "{}", err.message());
    assert_eq!(field(&account(&facade, "SSO_ROOT"), "externalUserId"), "");
}

#[test]
fn a_trusted_provider_s_verified_address_is_recorded_at_sign_in() {
    let (facade, context) = entered();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    crate::service::load_definitions().unwrap();
    let flow = flow(&facade);
    let plain = seed(
        &facade,
        "moqui.security.sso.AuthFlow",
        &[("authFlowId", Value::string("PLAIN_FLOW"))],
    );
    // accounts their provider already signs in, each with an address
    let linked = |user_id: &str, username: &str, email: &str, flow_id: &str| {
        seed(
            &facade,
            ACCOUNT,
            &[
                ("userId", Value::string(user_id)),
                ("username", Value::string(username)),
                ("emailAddress", Value::string(email)),
                ("externalUserId", Value::string(format!("sub-{username}"))),
                ("externalAuthFlowId", Value::string(flow_id)),
            ],
        );
    };
    let verified = |user_id: &str| -> Vec<(String, bool)> {
        let rows = facade
            .find("moqui.security.public.VerifiedAddress")
            .unwrap()
            .disable_authz()
            .eq("userId", Value::string(user_id))
            .use_cache(false)
            .list()
            .unwrap();
        rows.iter()
            .map(|row| (field(row, "emailAddress"), !field(row, "verifiedAt").is_empty()))
            .collect()
    };
    let sign_in = |flow: &EntityValue, username: &str, change: &dyn Fn(&mut SsoProfile)| {
        let mut given = profile(&format!("sub-{username}"), username, &[]);
        change(&mut given);
        sync_account(&facade, flow, &given, username).unwrap();
    };

    // the provider's verified address is the account's (whatever its case): recorded as the account has it, once
    linked("VA_ANN", "va.ann", "Va.Ann@Example.com", "TEST_FLOW");
    sign_in(&flow, "va.ann", &|_| {});
    assert_eq!(verified("VA_ANN"), [("Va.Ann@Example.com".to_string(), true)]);
    sign_in(&flow, "va.ann", &|_| {});
    assert_eq!(verified("VA_ANN").len(), 1);
    // an account the rule links by that address has it recorded by the same sign-in
    seed(
        &facade,
        ACCOUNT,
        &[
            ("userId", Value::string("VA_NEW")),
            ("username", Value::string("va.new")),
            ("emailAddress", Value::string("va.new@example.com")),
        ],
    );
    sign_in(&flow, "va.new", &|_| {});
    assert_eq!(verified("VA_NEW"), [("va.new@example.com".to_string(), true)]);

    // nothing is recorded for an address the provider did not verify, or another address than the account's
    linked("VA_BOB", "va.bob", "va.bob@example.com", "TEST_FLOW");
    sign_in(&flow, "va.bob", &|given| {
        given
            .attributes
            .insert("email_verified".to_string(), Value::Bool(false));
    });
    sign_in(&flow, "va.bob", &|given| {
        given
            .attributes
            .insert("email".to_string(), Value::string("someone.else@example.com"));
    });
    sign_in(&flow, "va.bob", &|given| {
        given.attributes.remove("email");
    });
    assert!(verified("VA_BOB").is_empty(), "{:?}", verified("VA_BOB"));
    // a row that is there is left as it is (here one with no time), and an account that is not is nothing to record
    linked("VA_DEE", "va.dee", "va.dee@example.com", "TEST_FLOW");
    seed(
        &facade,
        "moqui.security.public.VerifiedAddress",
        &[
            ("userId", Value::string("VA_DEE")),
            ("emailAddress", Value::string("va.dee@example.com")),
        ],
    );
    sign_in(&flow, "va.dee", &|_| {});
    assert_eq!(
        verified("VA_DEE"),
        [("va.dee@example.com".to_string(), false)],
        "a row there is left as it is"
    );
    assert!(super::note_verified_address(
        &facade,
        &flow,
        &profile("sub-none", "no.such.account", &[]),
        "no.such.account"
    )
    .is_ok());
    // nor under a flow whose rule does not trust the provider's address
    linked("VA_CAL", "va.cal", "va.cal@example.com", "PLAIN_FLOW");
    sign_in(&plain, "va.cal", &|_| {});
    assert!(verified("VA_CAL").is_empty(), "{:?}", verified("VA_CAL"));
}

#[test]
fn a_sign_in_gives_and_ends_only_the_groups_its_role_maps_may() {
    let (facade, context) = entered();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    crate::service::load_definitions().unwrap();
    let flow = seed(
        &facade,
        "moqui.security.sso.AuthFlow",
        &[
            ("authFlowId", Value::string("BOUND_FLOW")),
            ("defaultUserGroupId", Value::string("ADMIN")),
        ],
    );
    // staff maps an ordinary group, root a privileged one and sealed a group that grants a sealed permission
    seed(
        &facade,
        "moqui.security.UserGroupPermission",
        &[
            ("userGroupId", Value::string("SSO_SEALED")),
            ("userPermissionId", Value::string("ADMIN_PASSWORD")),
            ("fromDate", crate::user::now_value()),
        ],
    );
    for (role, group) in [("staff", "SSO_STAFF"), ("root", "ADMIN"), ("sealed", "SSO_SEALED")] {
        seed(
            &facade,
            "moqui.security.sso.AuthFlowRoleMap",
            &[
                ("authFlowId", Value::string("BOUND_FLOW")),
                ("roleName", Value::string(role)),
                ("userGroupId", Value::string(group)),
            ],
        );
    }
    seed(
        &facade,
        ACCOUNT,
        &[
            ("userId", Value::string("SSO_DEE")),
            ("username", Value::string("sso.dee")),
            ("externalUserId", Value::string("sub-dee")),
        ],
    );
    for group in ["ADMIN", "BY_HAND"] {
        seed(
            &facade,
            "moqui.security.UserGroupMember",
            &[
                ("userGroupId", Value::string(group)),
                ("userId", Value::string("SSO_DEE")),
                ("fromDate", crate::user::now_value()),
            ],
        );
    }
    // Every role: only the ordinary group is given; the privileged and the sealed ones are passed over.
    sync_account(
        &facade,
        &flow,
        &profile("sub-dee", "sso.dee", &["staff", "root", "sealed"]),
        "sso.dee",
    )
    .unwrap();
    assert_eq!(groups_of(&facade, "SSO_DEE"), ["ADMIN", "BY_HAND", "SSO_STAFF"]);
    // No role: the mapped group ends; the administrator's own membership and the one given by hand stay.
    sync_account(&facade, &flow, &profile("sub-dee", "sso.dee", &[]), "sso.dee").unwrap();
    assert_eq!(groups_of(&facade, "SSO_DEE"), ["ADMIN", "BY_HAND"]);
    // A new account with no role gets no privileged default group.
    sync_account(&facade, &flow, &profile("sub-eve", "sso.eve", &["root"]), "sso.eve").unwrap();
    let eve = facade
        .find(ACCOUNT)
        .unwrap()
        .disable_authz()
        .eq("username", Value::string("sso.eve"))
        .one()
        .unwrap()
        .unwrap();
    assert!(groups_of(&facade, &field(&eve, "userId")).is_empty());
}

/// Admin P10 review (F-P10-2): a provider's id is linked with its provider, so a second provider never signs in the
/// first one's account by asserting its id; a closed account is refused before anything is written; a plain mapped name
/// is a claim and nothing else.
#[test]
fn a_provider_signs_in_only_the_accounts_linked_to_it_and_no_closed_one() {
    let (facade, context) = entered();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    crate::service::load_definitions().unwrap();
    let first = flow(&facade);
    let mut second = seed(
        &facade,
        "moqui.security.sso.AuthFlow",
        &[("authFlowId", Value::string("OTHER_FLOW"))],
    );
    let linked = |user_id: &str, username: &str, id: &str, flow_id: Option<&str>| {
        let mut fields = vec![
            ("userId", Value::string(user_id)),
            ("username", Value::string(username)),
            ("externalUserId", Value::string(id)),
        ];
        if let Some(flow_id) = flow_id {
            fields.push(("externalAuthFlowId", Value::string(flow_id)));
        }
        seed(&facade, ACCOUNT, &fields);
    };
    let flow_of = |user_id: &str| field(&account(&facade, user_id), "externalAuthFlowId");
    // The first provider's account: the second one asserting its id and its name is refused, and with a name of its own
    // makes an account of its own.
    linked("SSO_FAY", "sso.fay", "sub-1001", Some("TEST_FLOW"));
    let err = sync_account(&facade, &second, &profile("sub-1001", "sso.fay", &[]), "sso.fay").unwrap_err();
    assert!(
        err.message().contains("linked to another provider"),
        "{}",
        err.message()
    );
    let username = sync_account(&facade, &second, &profile("sub-1001", "other.fay", &[]), "other.fay").unwrap();
    assert_eq!(username, "other.fay");
    assert_eq!(field(&account(&facade, "SSO_FAY"), "userFullName"), "");
    // Each provider then signs in its own account by the same id.
    assert_eq!(
        sync_account(&facade, &first, &profile("sub-1001", "renamed", &[]), "renamed").unwrap(),
        "sso.fay"
    );
    assert_eq!(
        sync_account(&facade, &second, &profile("sub-1001", "renamed", &[]), "renamed").unwrap(),
        "other.fay"
    );
    // An account with an id and no provider named: an ordinary one is taken by the provider that signs in with it; an
    // administrator's is not while several providers are enabled, and is once this is the only one.
    linked("SSO_GUS", "sso.gus", "sub-gus", None);
    sync_account(&facade, &second, &profile("sub-gus", "sso.gus", &[]), "sso.gus").unwrap();
    assert_eq!(flow_of("SSO_GUS"), "OTHER_FLOW");
    assert!(sync_account(&facade, &first, &profile("sub-gus", "x.gus", &[]), "x.gus").unwrap() != "sso.gus");
    linked("SSO_HAL", "sso.hal", "sub-hal", None);
    seed(
        &facade,
        "moqui.security.UserGroupMember",
        &[
            ("userGroupId", Value::string("ADMIN")),
            ("userId", Value::string("SSO_HAL")),
            ("fromDate", crate::user::now_value()),
        ],
    );
    let err = sync_account(&facade, &first, &profile("sub-hal", "sso.hal", &[]), "sso.hal").unwrap_err();
    assert!(err.message().contains("names no provider"), "{}", err.message());
    assert_eq!(flow_of("SSO_HAL"), "");
    second.set("disabled", Value::string("Y"));
    second.update(&facade).unwrap();
    sync_account(&facade, &first, &profile("sub-hal", "sso.hal", &[]), "sso.hal").unwrap();
    assert_eq!(flow_of("SSO_HAL"), "TEST_FLOW");
    // A closed account is refused with nothing written: no name, no group.
    seed(
        &facade,
        ACCOUNT,
        &[
            ("userId", Value::string("SSO_IDA")),
            ("username", Value::string("sso.ida")),
            ("externalUserId", Value::string("sub-ida")),
            ("externalAuthFlowId", Value::string("TEST_FLOW")),
            ("disabled", Value::string("Y")),
        ],
    );
    let err = sync_account(&facade, &first, &profile("sub-ida", "sso.ida", &["a"]), "sso.ida").unwrap_err();
    assert!(err.message().contains("disabled or ended"), "{}", err.message());
    assert_eq!(field(&account(&facade, "SSO_IDA"), "userFullName"), "");
    assert!(groups_of(&facade, "SSO_IDA").is_empty());
    // A plain mapped name is the claim of that name: one the provider did not send is no value, whatever the script
    // context or the engine has by that name.
    for (seq, name) in [("01", "ec"), ("02", "globalThis"), ("03", "locale")] {
        seed(
            &facade,
            "moqui.security.sso.AuthFlowFieldMap",
            &[
                ("authFlowId", Value::string("OTHER_FLOW")),
                ("ruleSeqId", Value::string(seq)),
                ("srcFieldName", Value::string(name)),
                ("dstFieldName", Value::string(name)),
            ],
        );
    }
    let mut claims = BTreeMap::new();
    claims.insert("locale".to_string(), Value::string("fr"));
    let mapped = map_fields(&facade, "OTHER_FLOW", &claims).unwrap();
    assert_eq!(mapped.keys().collect::<Vec<_>>(), ["locale"]);
}
