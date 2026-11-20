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

//! M3's Tests column (plans/Vault-Instances.md §6), against the fake engine and a fake of the manager link: the gate
//! refuses a commit with no passed run; a deploy uses only its lease's items; members change one at a time, each
//! drained; a rollback across a schema hash change is refused.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::Duration;

use moqui_core::entity::EntityFacade;
use moqui_lib::docker::fake::FakeEngine;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_vault_server::lease::{self, Ask, LeaseRegistry};
use serde_json::json;

use super::provision::{fleet, Fleet};
use super::{create, field_of, login, sys};
use crate::deploy::{self, Change, Component, Link, LinkStatus, Request};
use crate::services::NewInstance;

const INSTANCE: &str = "moqui.server.instance.AppInstance";
const ACTION: &str = "moqui.server.instance.InstanceAction";
const COMMIT_PASSED: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";
const COMMIT_FAILED: &str = "f0e1d2c3b4a5968778695a4b3c2d1e0f12345678";
const COMMIT_UNRUN: &str = "0000111122223333444455556666777788889999";

/// The manager link as the deploy sees it: each call recorded in order, each instance's schema hash, which a
/// component's deploy changes when `next_schema` is set, and whether it is drained.
#[derive(Default)]
struct FakeLink {
    calls: Mutex<Vec<String>>,
    schema: Mutex<BTreeMap<String, String>>,
    next_schema: Mutex<Option<String>>,
    drained: Mutex<BTreeSet<String>>,
}

impl FakeLink {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn position(&self, call: &str) -> usize {
        let calls = self.calls();
        calls
            .iter()
            .position(|known| known == call)
            .unwrap_or_else(|| panic!("no call {call} in {calls:?}"))
    }
}

impl Link for FakeLink {
    fn status(&self, instance_id: &str) -> Result<LinkStatus, LibError> {
        self.calls.lock().unwrap().push(format!("status {instance_id}"));
        let drained = self.drained.lock().unwrap().contains(instance_id);
        Ok(LinkStatus {
            seal_state: "unsealed".to_string(),
            health: if drained { "draining" } else { "ok" }.to_string(),
            schema_hash: self
                .schema
                .lock()
                .unwrap()
                .get(instance_id)
                .cloned()
                .unwrap_or_else(|| "schema-1".to_string()),
        })
    }

    fn drain(&self, instance_id: &str, drained: bool) -> Result<(), LibError> {
        let verb = if drained { "drain" } else { "undrain" };
        self.calls.lock().unwrap().push(format!("{verb} {instance_id}"));
        let mut set = self.drained.lock().unwrap();
        if drained {
            set.insert(instance_id.to_string());
        } else {
            set.remove(instance_id);
        }
        Ok(())
    }

    fn deploy_component(&self, instance_id: &str, attestation: &str) -> Result<String, LibError> {
        let given: serde_json::Value = serde_json::from_str(attestation).unwrap();
        // the link's deploy names the commit, unsigned: no attestation key on the instance
        assert_eq!(given["kind"], "moqui-deploy-attestation");
        assert!(given.get("signature").is_none());
        let drained = self.drained.lock().unwrap().contains(instance_id);
        self.calls.lock().unwrap().push(format!(
            "deploy {instance_id} {} drained={drained}",
            given["commitId"].as_str().unwrap_or_default()
        ));
        if let Some(next) = self.next_schema.lock().unwrap().clone() {
            self.schema.lock().unwrap().insert(instance_id.to_string(), next);
        }
        Ok(String::new())
    }
}

/// An mr-moqui instance of the fleet, provisioned on the fake engine (embedded database: no backup) and paired.
fn app(facade: &EntityFacade, fleet: &Fleet, registry: &LeaseRegistry, name: &str, cluster: &str) -> String {
    let id = fleet.instance(
        facade,
        NewInstance {
            instance_image_id: "MoquiLocal".to_string(),
            instance_name: name.to_string(),
            cluster_id: cluster.to_string(),
            ..NewInstance::default()
        },
    );
    crate::provision::provision(facade, registry, &id, None).unwrap();
    sys(|| {
        let mut row = facade
            .find(INSTANCE)
            .unwrap()
            .eq("appInstanceId", Value::string(&id))
            .one()
            .unwrap()
            .unwrap();
        row.set("linkPairedDate", moqui_core::user::now_value());
        row.set("schemaHash", Value::string("schema-1"));
        row.update(facade).unwrap();
    });
    id
}

/// A run of `pipeline` for `commit` that ended `status`.
fn run(facade: &EntityFacade, id: &str, pipeline: &str, commit: &str, status: &str) {
    create(
        facade,
        "moqui.resource.GitPipelineRun",
        json!({ "pipelineRunId": id, "repositoryId": "site", "pipelineName": pipeline, "commitId": commit,
                "statusId": status }),
    );
}

fn gated(facade: &EntityFacade, fleet: &Fleet) {
    sys(|| {
        let mut row = facade
            .find("moqui.server.instance.InstanceEnvironment")
            .unwrap()
            .eq("environmentId", Value::string(&fleet.environment))
            .one()
            .unwrap()
            .unwrap();
        row.set("requiredPipeline", Value::string("release"));
        row.update(facade).unwrap();
    });
    run(facade, "run-passed", "release", COMMIT_PASSED, "success");
    run(facade, "run-failed", "release", COMMIT_FAILED, "failure");
}

fn component_deploy(fleet: &Fleet, commit: &str) -> Request {
    let mut request = Request::new(
        &fleet.environment,
        Change::Component(Component {
            name: "site".to_string(),
            repository_url: "https://source.example.test/git/site.git".to_string(),
            ref_name: "refs/heads/main".to_string(),
            commit_id: commit.to_string(),
            data_types: Vec::new(),
        }),
    );
    request.commit_id = commit.to_string();
    request.repository_id = "site".to_string();
    request
}

/// The actions of an instance, as (action, state).
fn actions_of(facade: &EntityFacade, instance_id: &str) -> Vec<(String, String)> {
    sys(|| {
        facade
            .find(ACTION)
            .unwrap()
            .eq("appInstanceId", Value::string(instance_id))
            .order_by("instanceActionId", false)
            .list()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    crate::common::field(row, "actionEnumId"),
                    crate::common::field(row, "statusEnumId"),
                )
            })
            .collect()
    })
}

/// The last action of an instance.
fn last_action(facade: &EntityFacade, instance_id: &str) -> moqui_core::entity::EntityValue {
    sys(|| {
        facade
            .find(ACTION)
            .unwrap()
            .eq("appInstanceId", Value::string(instance_id))
            .order_by("instanceActionId", true)
            .list()
            .unwrap()
            .into_values()
            .into_iter()
            .next()
            .unwrap()
    })
}

#[test]
fn the_gate_refuses_a_commit_with_no_passed_run() {
    let (ctx, facade) = super::setup();
    let _guard = ctx.enter();
    let fleet = fleet(&facade, "gate", FakeEngine::docker());
    login(&fleet.vault.maker.user);
    let registry = lease::registry(ctx.factory());
    let app = app(&facade, &fleet, &registry, "app-gate", "");
    let link = FakeLink::default();

    // an environment that names no required pipeline deploys nothing
    let refused = deploy::run(
        &facade,
        &registry,
        &link,
        &component_deploy(&fleet, COMMIT_PASSED),
        None,
    )
    .unwrap_err();
    assert!(refused.message().contains("noGate"), "{refused}");

    gated(&facade, &fleet);
    for commit in [COMMIT_UNRUN, COMMIT_FAILED] {
        let refused = deploy::run(&facade, &registry, &link, &component_deploy(&fleet, commit), None).unwrap_err();
        assert!(refused.message().contains("notPassed"), "{commit}: {refused}");
    }
    // a run of another repository does not pass it either
    let mut other = component_deploy(&fleet, COMMIT_PASSED);
    other.repository_id = "another".to_string();
    assert!(deploy::run(&facade, &registry, &link, &other, None)
        .unwrap_err()
        .message()
        .contains("notPassed"));
    // nothing reached the instance, and each refusal is an action
    assert!(link.calls().is_empty(), "{:?}", link.calls());
    let refusals = actions_of(&facade, &app)
        .into_iter()
        .filter(|(action, state)| action == "IatDeploy" && state == "IasRefused")
        .count();
    assert_eq!(refusals, 4);

    // the commit its pipeline passed deploys, over the link
    let outcome = deploy::run(
        &facade,
        &registry,
        &link,
        &component_deploy(&fleet, COMMIT_PASSED),
        None,
    )
    .unwrap();
    assert_eq!(outcome.instance_ids, std::slice::from_ref(&app));
    assert!(link
        .calls()
        .contains(&format!("deploy {app} {COMMIT_PASSED} drained=false")));
    let commits = field_of(&facade, INSTANCE, "appInstanceId", &app, "componentCommits");
    assert!(commits.contains(COMMIT_PASSED), "{commits}");
}

#[test]
fn members_change_one_at_a_time_each_drained() {
    let (ctx, facade) = super::setup();
    let _guard = ctx.enter();
    let fleet = fleet(&facade, "roll", FakeEngine::docker());
    create(
        &facade,
        "moqui.server.instance.InstanceCluster",
        json!({ "clusterId": "cluster-roll", "environmentId": fleet.environment }),
    );
    login(&fleet.vault.maker.user);
    let registry = lease::registry(ctx.factory());
    let first = app(&facade, &fleet, &registry, "app-roll-1", "cluster-roll");
    let second = app(&facade, &fleet, &registry, "app-roll-2", "cluster-roll");
    let single = app(&facade, &fleet, &registry, "app-roll-single", "");
    gated(&facade, &fleet);
    let link = FakeLink::default();

    let plan = deploy::plan(&facade, &component_deploy(&fleet, COMMIT_PASSED)).unwrap();
    let order: Vec<String> = plan.targets.iter().map(deploy::Target::id).collect();
    assert_eq!(order, [single.clone(), first.clone(), second.clone()]);
    // a deploy over the link opens no vault item: no lease
    assert!(plan.bindings.is_empty(), "{:?}", plan.bindings);

    let outcome = deploy::run(
        &facade,
        &registry,
        &link,
        &component_deploy(&fleet, COMMIT_PASSED),
        None,
    )
    .unwrap();
    assert_eq!(outcome.instance_ids, order);
    // each member is drained, changed while drained, up, and undrained before the next is drained
    for member in [&first, &second] {
        let drain = link.position(&format!("drain {member}"));
        let change = link.position(&format!("deploy {member} {COMMIT_PASSED} drained=true"));
        let undrain = link.position(&format!("undrain {member}"));
        assert!(drain < change && change < undrain, "{:?}", link.calls());
        let up = link.calls()[change..undrain]
            .iter()
            .any(|call| *call == format!("status {member}"));
        assert!(up, "{member} reported up before its undrain: {:?}", link.calls());
    }
    assert!(link.position(&format!("undrain {first}")) < link.position(&format!("drain {second}")));
    // a single instance is changed in place, not drained
    assert!(link
        .calls()
        .contains(&format!("deploy {single} {COMMIT_PASSED} drained=false")));
    assert!(!link.calls().contains(&format!("drain {single}")));
    // each change is its action, with its versions before and after
    for member in [&first, &second, &single] {
        let action = last_action(&facade, member);
        assert_eq!(crate::common::field(&action, "actionEnumId"), "IatDeploy");
        assert_eq!(crate::common::field(&action, "statusEnumId"), "IasDone");
        assert!(crate::common::field(&action, "componentCommitsAfter").contains(COMMIT_PASSED));
        assert_eq!(crate::common::field(&action, "componentCommitsBefore"), "{}");
        assert_eq!(crate::common::field(&action, "schemaHashBefore"), "schema-1");
    }
}

#[test]
fn a_rollback_across_a_schema_hash_change_is_refused() {
    let (ctx, facade) = super::setup();
    let _guard = ctx.enter();
    let fleet = fleet(&facade, "back", FakeEngine::docker());
    login(&fleet.vault.maker.user);
    let registry = lease::registry(ctx.factory());
    let app = app(&facade, &fleet, &registry, "app-back", "");
    gated(&facade, &fleet);
    run(&facade, "run-second", "release", COMMIT_UNRUN, "success");
    let link = FakeLink::default();

    // a deploy that leaves the schema as it was rolls back to the commit before it
    deploy::run(
        &facade,
        &registry,
        &link,
        &component_deploy(&fleet, COMMIT_PASSED),
        None,
    )
    .unwrap();
    deploy::run(&facade, &registry, &link, &component_deploy(&fleet, COMMIT_UNRUN), None).unwrap();
    let second = crate::common::field(&last_action(&facade, &app), "instanceActionId");
    let mut rollback = Request::new(&fleet.environment, Change::Rollback { action_id: second });
    rollback.instance_id = app.clone();
    deploy::run(&facade, &registry, &link, &rollback, None).unwrap();
    assert_eq!(
        link.calls().iter().rfind(|call| call.starts_with("deploy ")).unwrap(),
        &format!("deploy {app} {COMMIT_PASSED} drained=false")
    );
    let action = last_action(&facade, &app);
    assert_eq!(crate::common::field(&action, "actionEnumId"), "IatRollback");
    assert_eq!(crate::common::field(&action, "statusEnumId"), "IasDone");
    assert!(field_of(&facade, INSTANCE, "appInstanceId", &app, "componentCommits").contains(COMMIT_PASSED));

    // a deploy that changes the schema hash is not rolled back
    *link.next_schema.lock().unwrap() = Some("schema-2".to_string());
    deploy::run(&facade, &registry, &link, &component_deploy(&fleet, COMMIT_UNRUN), None).unwrap();
    let changed = last_action(&facade, &app);
    assert_eq!(crate::common::field(&changed, "schemaHashBefore"), "schema-1");
    assert_eq!(crate::common::field(&changed, "schemaHashAfter"), "schema-2");
    let deploys_before = link.calls().len();
    let mut rollback = Request::new(
        &fleet.environment,
        Change::Rollback {
            action_id: crate::common::field(&changed, "instanceActionId"),
        },
    );
    rollback.instance_id = app.clone();
    let refused = deploy::run(&facade, &registry, &link, &rollback, None).unwrap_err();
    assert!(refused.message().contains("schemaChanged"), "{refused}");
    // the instance action `rollback` (Monitor's recovery adapter asks for it) is the rollback of the last deploy:
    // refused the same way
    let refused = crate::actions::needs(&facade, &app, "rollback").unwrap_err();
    assert!(refused.message().contains("schemaChanged"), "{refused}");
    assert_eq!(crate::actions::verb("IatRollback").unwrap(), "rollback");
    assert_eq!(link.calls().len(), deploys_before, "nothing reached the instance");
}

/// A lease of the deploy's operation, asked and approved.
fn approved(
    facade: &EntityFacade,
    fleet: &mut Fleet,
    registry: &LeaseRegistry,
    bindings: &[String],
    operation: &str,
) -> String {
    login(&fleet.vault.maker.user);
    let asked = registry
        .ask(
            facade,
            &Ask {
                vault_id: &fleet.vault.vault_id,
                bindings,
                operation,
                limit: Some(Duration::from_secs(600)),
                environment_id: None,
            },
        )
        .unwrap();
    fleet.vault.approve(&asked.lease_id);
    login(&fleet.vault.maker.user);
    asked.lease_id
}

/// The items a lease was used for, by its `VeLeaseUsed` events.
fn used_items(facade: &EntityFacade, lease_id: &str) -> BTreeSet<String> {
    sys(|| {
        facade
            .find("moqui.security.vault.VaultEvent")
            .unwrap()
            .eq("leaseId", Value::string(lease_id))
            .eq("eventTypeEnumId", Value::string("VeLeaseUsed"))
            .list()
            .unwrap()
            .iter()
            .map(|row| crate::common::field(row, "vaultItemId"))
            .collect()
    })
}

#[test]
fn a_deploy_uses_only_its_leases_items() {
    let (ctx, facade) = super::setup();
    let _guard = ctx.enter();
    let mut fleet = fleet(&facade, "items", FakeEngine::docker());
    login(&fleet.vault.maker.user);
    let registry = lease::registry(ctx.factory());
    let app = app(&facade, &fleet, &registry, "app-items", "");
    gated(&facade, &fleet);
    // a new image of its kind from a registry that needs a login, which is a vault item
    create(
        &facade,
        "moqui.server.instance.InstanceImage",
        json!({ "instanceImageId": "MoquiNext", "instanceKindId": "mr-moqui", "imageName": "registry.example.com/moqui:2",
                "registryLocation": "registry.example.com", "registryLogin": "Y" }),
    );
    fleet.vault.item(
        "registry-login",
        "InstanceImage:MoquiNext:registry",
        &json!({ "registry": "registry.example.com", "username": "puller", "password": "a-registry-token-value" }),
    );
    // a second item of the vault, which the deploy has no use for
    fleet.vault.item(
        "registry-login",
        "InstanceImage:Unused:registry",
        &json!({ "registry": "registry.example.com", "username": "other", "password": "another-token-value" }),
    );
    let mut request = Request::new(
        &fleet.environment,
        Change::Image {
            image_id: "MoquiNext".to_string(),
        },
    );
    request.commit_id = COMMIT_PASSED.to_string();
    let link = FakeLink::default();
    let plan = deploy::plan(&facade, &request).unwrap();
    assert_eq!(plan.bindings, ["InstanceImage:MoquiNext:registry".to_string()]);
    assert!(
        plan.operation
            .contains("InstanceImage:MoquiNext:registry to registry.example.com"),
        "{}",
        plan.operation
    );
    let before = field_of(&facade, INSTANCE, "appInstanceId", &app, "instanceUuid");

    // no lease and no machine access: refused, and the refusal is an action
    let refused = deploy::run(&facade, &registry, &link, &request, None).unwrap_err();
    assert!(refused.message().contains("leaseRequired"), "{refused}");
    // a lease of the same items for another operation is refused before any use
    let other = approved(
        &facade,
        &mut fleet,
        &registry,
        &plan.bindings,
        &format!("provision AppInstance {app}"),
    );
    let refused = deploy::run(&facade, &registry, &link, &request, Some(&other)).unwrap_err();
    assert!(refused.message().contains("not \"deploy image MoquiNext"), "{refused}");
    assert!(used_items(&facade, &other).is_empty());
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", &app, "instanceUuid"),
        before
    );

    // its own lease: the container is replaced from the new image, and the lease was used for its one item only
    let own = approved(&facade, &mut fleet, &registry, &plan.bindings, &plan.operation);
    let outcome = deploy::run(&facade, &registry, &link, &request, Some(&own)).unwrap();
    assert_eq!(outcome.instance_ids, std::slice::from_ref(&app));
    let after = field_of(&facade, INSTANCE, "appInstanceId", &app, "instanceUuid");
    assert!(!after.is_empty() && after != before);
    assert!(fleet.engine.removed().iter().any(|(id, _)| *id == before));
    // the new container reports the new version, for Monitor's Releases
    let body = fleet.engine.containers()[&after].body.to_string();
    assert!(body.contains("moqui_conf__telemetry__service_version=2"), "{body}");
    let item = field_of(
        &facade,
        "moqui.security.vault.VaultItem",
        "bindingKey",
        "InstanceImage:MoquiNext:registry",
        "vaultItemId",
    );
    assert_eq!(used_items(&facade, &own), BTreeSet::from([item]));
    // the login went to the registry only, never into a container
    assert!(fleet.engine.secret_outside_files(b"a-registry-token-value").is_empty());
    // the lease ended with the deploy: it opens nothing again
    assert!(!registry.holds(&own));
    let action = last_action(&facade, &app);
    assert_eq!(crate::common::field(&action, "leaseId"), own);
    assert_eq!(crate::common::field(&action, "statusEnumId"), "IasDone");
}

/// A deploy and a rollback are checked as their caller, before any authorization is turned off: an INSTANCE_ADMIN who
/// is not in the environment's approver group is refused both, through the services as a screen calls them.
#[test]
fn a_deploy_and_a_rollback_need_the_environments_approver_group() {
    let (ctx, facade) = super::setup();
    let _guard = ctx.enter();
    let fleet = fleet(&facade, "group", FakeEngine::docker());
    login(&fleet.vault.maker.user);
    let registry = lease::registry(ctx.factory());
    let app = app(&facade, &fleet, &registry, "app-group", "");
    gated(&facade, &fleet);
    deploy::run(
        &facade,
        &registry,
        &FakeLink::default(),
        &component_deploy(&fleet, COMMIT_PASSED),
        None,
    )
    .unwrap();
    let deployed = crate::common::field(&last_action(&facade, &app), "instanceActionId");
    create(
        &facade,
        "moqui.security.UserGroup",
        json!({ "userGroupId": "IT_DEPLOY_APPROVERS" }),
    );
    sys(|| {
        let mut row = facade
            .find("moqui.server.instance.InstanceEnvironment")
            .unwrap()
            .eq("environmentId", Value::string(&fleet.environment))
            .one()
            .unwrap()
            .unwrap();
        row.set("approverUserGroupId", Value::string("IT_DEPLOY_APPROVERS"));
        row.update(&facade).unwrap();
    });
    // the maker holds INSTANCE_ADMIN and is not in the group
    login(&fleet.vault.maker.user);
    let call = |service: &str, parameters: &[(&str, &str)]| -> String {
        let parameters: BTreeMap<String, Value> = parameters
            .iter()
            .map(|(name, value)| ((*name).to_string(), Value::string(*value)))
            .collect();
        let answer = moqui_core::service::sync(&format!("org.moqui.dev.InstanceServices.{service}"), &parameters);
        let errors = moqui_core::context::ExecutionContext::with_current(|ec| {
            let text = ec.message().errors().join("\n");
            ec.message().clear_errors();
            text
        })
        .unwrap_or_default();
        match answer {
            Err(err) => format!("{err} {errors}"),
            Ok(_) => errors,
        }
    };
    let actions_before = actions_of(&facade, &app).len();
    let refused = call(
        "request#InstanceDeploy",
        &[
            ("appInstanceId", app.as_str()),
            ("componentName", "site"),
            ("repositoryUrl", "https://source.example.test/git/site.git"),
            ("refName", "main"),
            ("commitId", COMMIT_PASSED),
        ],
    );
    assert!(refused.contains("notEnvironmentMember"), "{refused}");
    let refused = call(
        "request#InstanceDeploy",
        &[
            ("appInstanceId", app.as_str()),
            ("rollbackOfActionId", deployed.as_str()),
        ],
    );
    assert!(refused.contains("notEnvironmentMember"), "{refused}");
    let refused = call(
        "preview#InstanceDeploy",
        &[
            ("appInstanceId", app.as_str()),
            ("rollbackOfActionId", deployed.as_str()),
        ],
    );
    assert!(refused.contains("notEnvironmentMember"), "{refused}");
    assert_eq!(actions_of(&facade, &app).len(), actions_before, "nothing ran");
    // a member of the group passes the check: the plan then refuses on its own rule (the first deploy added the component)
    create(
        &facade,
        "moqui.security.UserGroupMember",
        json!({ "userGroupId": "IT_DEPLOY_APPROVERS", "userId": fleet.vault.maker.user, "fromDate": 0 }),
    );
    login(&fleet.vault.maker.user);
    let previewed = call(
        "preview#InstanceDeploy",
        &[
            ("appInstanceId", app.as_str()),
            ("rollbackOfActionId", deployed.as_str()),
        ],
    );
    assert!(
        !previewed.contains("notEnvironmentMember") && previewed.contains("does not remove a component"),
        "{previewed}"
    );
}
