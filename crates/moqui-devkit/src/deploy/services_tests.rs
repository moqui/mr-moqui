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

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_lib::object::Value;

const SECRET: &str = "issued-by-this-target-0123456789";

fn gate_on() -> String {
    format!(
        r#"<moqui-conf><component-deploy require-passed-run="true" attestation-secret="{SECRET}"
            public-urls="https://target.example.com"><allow component="site"
            repository-url="https://source.example.com/git/site.git" branch="main"/></component-deploy></moqui-conf>"#
    )
}

fn errors() -> String {
    ExecutionContext::with_current(|ec| ec.message().errors().join("; ")).unwrap_or_default()
}

/// `load#Component` loads a directory, which has no commit to attest: under the gate it is refused, even for an
/// administrator.
#[test]
fn load_component_is_refused_under_the_gate() {
    let context = ExecutionContextFactory::test_context_with_conf(&gate_on());
    let entity = moqui_core::testing::open();
    context.factory().set_entity(entity.clone());
    let _entered = context.enter();
    {
        let _authz = context.disable_authz();
        let mut account = entity.make_value("moqui.security.UserAccount").unwrap();
        account.set("userId", Value::string("GATE_ADMIN"));
        account.set("username", Value::string("gate.admin"));
        account.create(&entity).unwrap();
        let mut member = entity.make_value("moqui.security.UserGroupMember").unwrap();
        member.set("userId", Value::string("GATE_ADMIN"));
        member.set("userGroupId", Value::string("ADMIN"));
        member.set("fromDate", moqui_core::user::stamp_value(1_000));
        member.create(&entity).unwrap();
    }
    moqui_core::user::internal_login_user("gate.admin").unwrap();
    assert!(moqui_core::user::is_in_group("ADMIN").unwrap());
    let parameters = BTreeMap::from([("componentName".to_string(), Value::string("placed"))]);
    let out = super::load_component(&parameters, &entity).unwrap();
    assert!(!out.contains_key("report"), "{out:?}");
    assert_eq!(errors(), crate::deploy::GATE_REFUSAL);
}

/// `deploy#AttestedComponent` is called with no user signed in (REST `POST /rest/s1/moqui/components/deploy`, whose
/// resource needs no authentication; this test calls the service's function as that resource does, with no user, the
/// devkit's tests having no HTTP server): a forged attestation is the one generic refusal, and a good one is taken
/// with no user and goes on to its fetch (refused here by sim mode), its data types its own, not a parameter's.
#[cfg(feature = "git")]
#[test]
fn deploy_attested_component_takes_an_attestation_with_no_user_and_refuses_a_forged_one() {
    use moqui_git::ci::attest::{sign, Attestation, KIND, VERSION};
    let context = ExecutionContextFactory::test_context_with_conf(&gate_on());
    let entity = moqui_core::testing::open();
    context.factory().set_entity(entity.clone());
    let _entered = context.enter();
    assert_eq!(moqui_core::user::session::current_user_id().unwrap(), None);
    let now = moqui_core::service::util::now_millis();
    let attestation = Attestation {
        kind: KIND.to_string(),
        v: VERSION,
        audience: "https://target.example.com".to_string(),
        repository_url: "https://source.example.com/git/site.git".to_string(),
        ref_name: "refs/heads/main".to_string(),
        commit_id: "0123456789abcdef0123456789abcdef01234567".to_string(),
        component: "site".to_string(),
        pipeline: "release".to_string(),
        pipeline_run_id: "100".to_string(),
        run_num: 1,
        run_status: "success".to_string(),
        environment: "prod".to_string(),
        deployment_id: "D1".to_string(),
        issued_at: now,
        source: "https://source.example.com".to_string(),
        data_types: Vec::new(),
    };
    let call = |secret: &str| {
        let signed = sign(secret, &attestation).unwrap();
        let parameters = BTreeMap::from([
            ("attestation".to_string(), Value::string(&signed.text)),
            ("signature".to_string(), Value::string(&signed.signature)),
            // not signed: not read
            ("dataTypes".to_string(), Value::string("demo")),
        ]);
        ExecutionContext::with_current(|ec| ec.message().clear_errors());
        super::deploy_attested_component(&parameters, &entity).unwrap()
    };
    call("another-secret-entirely-0123456789");
    assert_eq!(errors(), "the attestation is not accepted");
    entity.set_sim_active(true);
    call(SECRET);
    entity.set_sim_active(false);
    let errors = errors();
    assert!(
        errors.contains("sim mode") || errors.contains("git resources are not installed"),
        "{errors}"
    );
}
