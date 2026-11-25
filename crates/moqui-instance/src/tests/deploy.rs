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

//! M3's Tests column (plans/archive/Vault-Instances.md §6), against the fake engine and a fake of the manager link: the gate
//! refuses what no passed run covers; a deploy uses only its lease's items; members change one at a time, each drained;
//! a rollback across a schema hash change, or with a schema not known, is refused. And what the review of M3 added: a
//! failed replacement leaves the old container running, the caller is checked against the environment's approver
//! group before any authorization is turned off, a deploy with no lease waits for another member, and the deployment
//! services are the deploy step's alone.

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
const SITE_URL: &str = "https://source.example.test/git/site.git";
const COMMIT_PASSED: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";
const COMMIT_FAILED: &str = "f0e1d2c3b4a5968778695a4b3c2d1e0f12345678";
const COMMIT_UNRUN: &str = "0000111122223333444455556666777788889999";

/// The manager link as the deploy sees it: each call recorded in order, each instance's schema hash (empty, not known,
/// for one in `unknown` until its first deploy), which a component's deploy changes when `next_schema` is set, and
/// whether it is drained.
#[derive(Default)]
struct FakeLink {
    calls: Mutex<Vec<String>>,
    schema: Mutex<BTreeMap<String, String>>,
    next_schema: Mutex<Option<String>>,
    unknown: Mutex<BTreeSet<String>>,
    drained: Mutex<BTreeSet<String>>,
}

impl FakeLink {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// The calls that change something: no status.
    fn changes(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|call| !call.starts_with("status "))
            .collect()
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
        let schema_hash = if self.unknown.lock().unwrap().contains(instance_id) {
            String::new()
        } else {
            self.schema
                .lock()
                .unwrap()
                .get(instance_id)
                .cloned()
                .unwrap_or_else(|| "schema-1".to_string())
        };
        Ok(LinkStatus {
            seal_state: "unsealed".to_string(),
            health: if drained { "draining" } else { "ok" }.to_string(),
            schema_hash,
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
        // the link's deploy names the commit, unsigned, and whether it is a rollback
        assert_eq!(given["kind"], "moqui-deploy-attestation");
        assert!(given.get("signature").is_none());
        let drained = self.drained.lock().unwrap().contains(instance_id);
        self.calls.lock().unwrap().push(format!(
            "deploy {instance_id} {} drained={drained} rollback={}",
            given["commitId"].as_str().unwrap_or_default(),
            given["rollback"]
        ));
        self.unknown.lock().unwrap().remove(instance_id);
        if let Some(next) = self.next_schema.lock().unwrap().clone() {
            self.schema.lock().unwrap().insert(instance_id.to_string(), next);
        }
        Ok(String::new())
    }
}

/// An mr-moqui instance of the fleet, provisioned on the fake engine (embedded database: no backup) and paired.
fn app(facade: &EntityFacade, fleet: &Fleet, registry: &LeaseRegistry, name: &str, cluster: &str) -> String {
    // a drained member's requests are not waited for in a test
    moqui_lib::env::set_property("moqui_conf__manage__drain_wait_seconds", "0");
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
    set_row(facade, INSTANCE, "appInstanceId", &id, &[("linkPairedDate", "now")]);
    id
}

fn set_row(facade: &EntityFacade, entity: &str, key: &str, id: &str, fields: &[(&str, &str)]) {
    sys(|| {
        let mut row = facade
            .find(entity)
            .unwrap()
            .eq(key, Value::string(id))
            .one()
            .unwrap()
            .unwrap();
        for (name, value) in fields {
            let value = match *value {
                "now" => moqui_core::user::now_value(),
                "" => Value::Null,
                other => Value::string(other),
            };
            row.set(*name, value);
        }
        row.update(facade).unwrap();
    });
}

/// A run of `pipeline` of the repository `site` for `commit` that ended `status`.
fn run(facade: &EntityFacade, id: &str, pipeline: &str, commit: &str, status: &str) {
    create(
        facade,
        "moqui.resource.GitPipelineRun",
        json!({ "pipelineRunId": id, "repositoryId": "site", "pipelineName": pipeline, "commitId": commit,
                "statusId": status }),
    );
}

/// The environment's required pipeline, its runs, and the CI environment of `site` that deploys to it.
fn gated(facade: &EntityFacade, fleet: &Fleet) {
    set_row(
        facade,
        "moqui.server.instance.InstanceEnvironment",
        "environmentId",
        &fleet.environment,
        &[("requiredPipeline", "release")],
    );
    run(facade, "run-passed", "release", COMMIT_PASSED, "success");
    run(facade, "run-failed", "release", COMMIT_FAILED, "failure");
    create(
        facade,
        "moqui.resource.GitCiEnvironment",
        json!({ "environmentId": format!("ci-{}", fleet.environment), "repositoryId": "site", "name": "prod",
                "instanceEnvironmentId": fleet.environment, "cloneUrl": SITE_URL }),
    );
}

fn component_deploy(fleet: &Fleet, commit: &str) -> Request {
    let mut request = Request::new(
        &fleet.environment,
        Change::Component(Component {
            name: "site".to_string(),
            repository_url: SITE_URL.to_string(),
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
fn the_gate_refuses_what_no_passed_run_covers() {
    let (ctx, facade) = super::setup();
    let _guard = ctx.enter();
    let fleet = fleet(&facade, "gate", FakeEngine::docker());
    login(&fleet.vault.maker.user);
    let registry = lease::registry(ctx.factory());
    let app = app(&facade, &fleet, &registry, "app-gate", "");
    let link = FakeLink::default();
    let refused = |request: &Request| {
        deploy::run(&facade, &registry, &link, request, None)
            .unwrap_err()
            .message()
            .to_string()
    };

    // an environment that names no required pipeline deploys nothing
    assert!(refused(&component_deploy(&fleet, COMMIT_PASSED)).contains("noGate"));
    gated(&facade, &fleet);
    for commit in [COMMIT_UNRUN, COMMIT_FAILED] {
        let why = refused(&component_deploy(&fleet, commit));
        assert!(why.contains("notPassed"), "{commit}: {why}");
    }
    // a component: its repository is named, a CI environment of it deploys here, and its URL is that one's
    let mut unnamed = component_deploy(&fleet, COMMIT_PASSED);
    unnamed.repository_id = String::new();
    assert!(refused(&unnamed).contains("noRepository"));
    let mut other = component_deploy(&fleet, COMMIT_PASSED);
    other.repository_id = "another".to_string();
    assert!(refused(&other).contains("notLinked"));
    let mut elsewhere = component_deploy(&fleet, COMMIT_PASSED);
    if let Change::Component(component) = &mut elsewhere.change {
        component.repository_url = "https://evil.example.test/site.git".to_string();
    }
    assert!(refused(&elsewhere).contains("urlMismatch"));
    // an image: gated on the revision its row records, never a commit the caller names
    create(
        &facade,
        "moqui.server.instance.InstanceImage",
        json!({ "instanceImageId": "MoquiUnrecorded", "instanceKindId": "mr-moqui", "imageName": "localhost/moqui:9" }),
    );
    let mut image = Request::new(
        &fleet.environment,
        Change::Image {
            image_id: "MoquiUnrecorded".to_string(),
        },
    );
    image.commit_id = COMMIT_PASSED.to_string();
    assert!(refused(&image).contains("noRevision"));
    set_row(
        &facade,
        "moqui.server.instance.InstanceImage",
        "instanceImageId",
        "MoquiUnrecorded",
        &[("sourceRepositoryId", "site"), ("sourceRevision", COMMIT_UNRUN)],
    );
    assert!(refused(&image).contains("notPassed"));
    // nothing reached the instance, and each refusal is an action
    assert!(link.changes().is_empty(), "{:?}", link.calls());
    let refusals = actions_of(&facade, &app)
        .into_iter()
        .filter(|(action, state)| action == "IatDeploy" && state == "IasRefused")
        .count();
    assert_eq!(refusals, 8);

    // what its pipeline passed deploys, over the link
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
        .contains(&format!("deploy {app} {COMMIT_PASSED} drained=false rollback=false")));
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

    let plan = deploy::plan(&facade, &link, &component_deploy(&fleet, COMMIT_PASSED)).unwrap();
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
    // each member is drained (and reports itself drained), changed while drained, up, and undrained before the next is
    // drained
    for member in [&first, &second] {
        let drain = link.position(&format!("drain {member}"));
        let change = link.position(&format!("deploy {member} {COMMIT_PASSED} drained=true rollback=false"));
        let undrain = link.position(&format!("undrain {member}"));
        assert!(drain < change && change < undrain, "{:?}", link.calls());
        let calls = link.calls();
        assert!(calls[drain..change].contains(&format!("status {member}")), "{calls:?}");
        assert!(
            calls[change..undrain].contains(&format!("status {member}")),
            "{calls:?}"
        );
    }
    assert!(link.position(&format!("undrain {first}")) < link.position(&format!("drain {second}")));
    // a single instance is changed in place, not drained
    assert!(link
        .calls()
        .contains(&format!("deploy {single} {COMMIT_PASSED} drained=false rollback=false")));
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
fn a_rollback_across_a_schema_hash_change_or_of_an_unknown_schema_is_refused() {
    let (ctx, facade) = super::setup();
    let _guard = ctx.enter();
    let fleet = fleet(&facade, "back", FakeEngine::docker());
    login(&fleet.vault.maker.user);
    let registry = lease::registry(ctx.factory());
    let app = app(&facade, &fleet, &registry, "app-back", "");
    gated(&facade, &fleet);
    run(&facade, "run-second", "release", COMMIT_UNRUN, "success");
    let link = FakeLink::default();
    let rollback_of = |action_id: String| {
        let mut rollback = Request::new(&fleet.environment, Change::Rollback { action_id });
        rollback.instance_id = app.clone();
        rollback
    };

    // the first deploy, when the instance's schema was not known yet: its rollback is refused
    link.unknown.lock().unwrap().insert(app.clone());
    deploy::run(
        &facade,
        &registry,
        &link,
        &component_deploy(&fleet, COMMIT_PASSED),
        None,
    )
    .unwrap();
    let first = last_action(&facade, &app);
    assert_eq!(crate::common::field(&first, "schemaHashBefore"), "");
    let refused = deploy::run(
        &facade,
        &registry,
        &link,
        &rollback_of(crate::common::field(&first, "instanceActionId")),
        None,
    )
    .unwrap_err();
    assert!(refused.message().contains("schemaChanged"), "{refused}");

    // a deploy that leaves the schema as it was rolls back to the commit before it, marked a rollback
    deploy::run(&facade, &registry, &link, &component_deploy(&fleet, COMMIT_UNRUN), None).unwrap();
    let second = crate::common::field(&last_action(&facade, &app), "instanceActionId");
    deploy::run(&facade, &registry, &link, &rollback_of(second), None).unwrap();
    assert_eq!(
        link.changes().last().unwrap(),
        &format!("deploy {app} {COMMIT_PASSED} drained=false rollback=true")
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
    let changes_before = link.changes().len();
    let refused = deploy::run(
        &facade,
        &registry,
        &link,
        &rollback_of(crate::common::field(&changed, "instanceActionId")),
        None,
    )
    .unwrap_err();
    assert!(refused.message().contains("schemaChanged"), "{refused}");
    assert_eq!(link.changes().len(), changes_before, "nothing reached the instance");
    // the instance action `rollback` (Monitor's recovery adapter asks for it) is the rollback of the last deploy:
    // refused the same way
    let refused = crate::actions::needs(&facade, &app, "rollback").unwrap_err();
    assert!(refused.message().contains("schemaChanged"), "{refused}");
    assert_eq!(crate::actions::verb("IatRollback").unwrap(), "rollback");
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

/// A member took in the item `binding` the provision deposited (here: made again by the vault's manager), so a lease can
/// name it: the ingest key a replaced container's file holds (M4).
fn take_in(facade: &EntityFacade, fleet: &mut Fleet, binding: &str) {
    sys(|| {
        facade
            .find("moqui.security.vault.VaultItem")
            .unwrap()
            .eq("bindingKey", Value::string(binding))
            .delete_all()
            .unwrap()
    });
    fleet.vault.item(
        "api-key",
        binding,
        &json!({ "keyId": "ingest", "secret": "an-ingest-key-value" }),
    );
}

/// A new image of the mr-moqui kind, its source revision recorded as passed.
fn image(facade: &EntityFacade, id: &str, name: &str, login: bool) {
    let mut fields = json!({ "instanceImageId": id, "instanceKindId": "mr-moqui", "imageName": name,
                             "sourceRepositoryId": "site", "sourceRevision": COMMIT_PASSED });
    if login {
        fields["registryLocation"] = json!("registry.example.com");
        fields["registryLogin"] = json!("Y");
    }
    create(facade, "moqui.server.instance.InstanceImage", fields);
}

#[test]
fn a_deploy_uses_only_its_leases_items() {
    let (ctx, facade) = super::setup();
    let _guard = ctx.enter();
    let mut fleet = fleet(&facade, "items", FakeEngine::docker());
    login(&fleet.vault.maker.user);
    let registry = lease::registry(ctx.factory());
    let app = app(&facade, &fleet, &registry, "app-items", "");
    take_in(&facade, &mut fleet, &format!("AppInstance:{app}:ingestKey"));
    gated(&facade, &fleet);
    // a new image of its kind from a registry that needs a login, which is a vault item
    image(&facade, "MoquiNext", "registry.example.com/moqui:2", true);
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
    let request = Request::new(
        &fleet.environment,
        Change::Image {
            image_id: "MoquiNext".to_string(),
        },
    );
    let link = FakeLink::default();
    let plan = deploy::plan(&facade, &link, &request).unwrap();
    // the new image's registry login, and the secret file the container is made with again (its ingest key)
    assert_eq!(
        plan.bindings,
        [
            "InstanceImage:MoquiNext:registry".to_string(),
            format!("AppInstance:{app}:ingestKey")
        ]
    );
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
    // the old one, set aside while the new one came up, is gone; the new one has the instance's name
    assert!(fleet.engine.removed().iter().any(|(id, _)| *id == before));
    assert_eq!(fleet.engine.containers()[&after].name, "app-items");
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", &app, "instanceImageId"),
        "MoquiNext"
    );
    // the new container reports the new version, for Monitor's Releases
    let body = fleet.engine.containers()[&after].body.to_string();
    assert!(body.contains("moqui_conf__telemetry__service_version=2"), "{body}");
    let item = |binding: &str| {
        field_of(
            &facade,
            "moqui.security.vault.VaultItem",
            "bindingKey",
            binding,
            "vaultItemId",
        )
    };
    assert_eq!(
        used_items(&facade, &own),
        BTreeSet::from([
            item("InstanceImage:MoquiNext:registry"),
            item(&format!("AppInstance:{app}:ingestKey"))
        ])
    );
    // the ingest key is a file of the new container, as it was of the old
    assert_eq!(
        fleet.engine.secret_outside_files(b"an-ingest-key-value"),
        Vec::<String>::new()
    );
    // the login went to the registry only, never into a container
    assert!(fleet.engine.secret_outside_files(b"a-registry-token-value").is_empty());
    // the lease ended with the deploy: it opens nothing again
    assert!(!registry.holds(&own));
    let action = last_action(&facade, &app);
    assert_eq!(crate::common::field(&action, "leaseId"), own);
    assert_eq!(crate::common::field(&action, "statusEnumId"), "IasDone");
    assert_eq!(crate::common::field(&action, "instanceImageIdBefore"), "MoquiLocal");
}

#[test]
fn a_replacement_that_fails_leaves_the_old_container_running() {
    let (ctx, facade) = super::setup();
    let _guard = ctx.enter();
    let mut fleet = fleet(&facade, "keep", FakeEngine::docker());
    login(&fleet.vault.maker.user);
    let registry = lease::registry(ctx.factory());
    let app = app(&facade, &fleet, &registry, "app-keep", "");
    take_in(&facade, &mut fleet, &format!("AppInstance:{app}:ingestKey"));
    gated(&facade, &fleet);
    // the fake engine refuses to make a container of the image `broken`
    image(&facade, "MoquiBroken", "broken", false);
    let before = field_of(&facade, INSTANCE, "appInstanceId", &app, "instanceUuid");
    let request = Request::new(
        &fleet.environment,
        Change::Image {
            image_id: "MoquiBroken".to_string(),
        },
    );
    let link = FakeLink::default();
    let plan = deploy::plan(&facade, &link, &request).unwrap();
    let lease_id = approved(&facade, &mut fleet, &registry, &plan.bindings, &plan.operation);
    let refused = deploy::run(&facade, &registry, &link, &request, Some(&lease_id)).unwrap_err();
    assert!(refused.message().contains("unwell"), "{refused}");
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", &app, "instanceUuid"),
        before
    );
    let old = &fleet.engine.containers()[&before];
    assert_eq!((old.name.as_str(), old.status.as_str()), ("app-keep", "running"));
    assert!(!fleet.engine.removed().iter().any(|(id, _)| *id == before));
    let action = last_action(&facade, &app);
    assert_eq!(crate::common::field(&action, "statusEnumId"), "IasFailed");
    assert!(crate::common::field(&action, "report").contains("its old one is back"));
}

/// Calls an instance service as the signed-in user; its errors, empty when none, and its results.
fn call(service: &str, parameters: &[(&str, &str)]) -> (String, BTreeMap<String, Value>) {
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
        Err(err) => (format!("{err} {errors}"), BTreeMap::new()),
        Ok(out) => (errors, out),
    }
}

/// A deploy and a rollback are checked as their caller, before any authorization is turned off: an INSTANCE_ADMIN who
/// is not in the environment's approver group is refused both, through the services as a screen calls them. A deploy
/// that opens no vault item waits for another member of the group, never its requester; and a deployment's prepare and
/// deploy are its deploy step's alone.
#[test]
fn a_deploy_needs_the_environments_approver_group_and_a_second_member() {
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
    set_row(
        &facade,
        "moqui.server.instance.InstanceEnvironment",
        "environmentId",
        &fleet.environment,
        &[("approverUserGroupId", "IT_DEPLOY_APPROVERS")],
    );
    // the maker holds INSTANCE_ADMIN and is not in the group
    login(&fleet.vault.maker.user);
    let deploy_params = [
        ("appInstanceId", app.as_str()),
        ("componentName", "site"),
        ("repositoryId", "site"),
        ("refName", "main"),
        ("commitId", COMMIT_PASSED),
    ];
    let rollback_params = [
        ("appInstanceId", app.as_str()),
        ("rollbackOfActionId", deployed.as_str()),
    ];
    let actions_before = actions_of(&facade, &app).len();
    for (service, parameters) in [
        ("request#InstanceDeploy", &deploy_params[..]),
        ("request#InstanceDeploy", &rollback_params[..]),
        ("preview#InstanceDeploy", &rollback_params[..]),
    ] {
        let (refused, _) = call(service, parameters);
        assert!(refused.contains("notEnvironmentMember"), "{service}: {refused}");
    }
    assert_eq!(actions_of(&facade, &app).len(), actions_before, "nothing ran");

    // a member asks: with no vault item to lease, the deploy waits for another member
    for user in [&fleet.vault.maker.user, &fleet.vault.approver.user] {
        create(
            &facade,
            "moqui.security.UserGroupMember",
            json!({ "userGroupId": "IT_DEPLOY_APPROVERS", "userId": user, "fromDate": 0 }),
        );
    }
    create(
        &facade,
        "moqui.security.UserGroupMember",
        json!({ "userGroupId": "IT_group", "userId": fleet.vault.approver.user, "fromDate": 0 }),
    );
    login(&fleet.vault.maker.user);
    let (errors, asked) = call("request#InstanceDeploy", &deploy_params);
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(asked["statusEnumId"], Value::string("IasWaiting"));
    let waiting = moqui_lib::object::to_plain_string(&asked["instanceActionId"]);
    // not by who asked for it
    let (refused, _) = call("approve#InstanceDeploy", &[("instanceActionId", waiting.as_str())]);
    assert!(refused.contains("selfApproval"), "{refused}");
    assert_eq!(
        field_of(&facade, ACTION, "instanceActionId", &waiting, "statusEnumId"),
        "IasWaiting"
    );
    // another member approves it: the deploy as asked runs (here with no link server, so it fails at the link)
    login(&fleet.vault.approver.user);
    let _ = call("approve#InstanceDeploy", &[("instanceActionId", waiting.as_str())]);
    let report = field_of(&facade, ACTION, "instanceActionId", &waiting, "report");
    assert!(
        report.contains(&format!("approved by {}", fleet.vault.approver.user)),
        "{report}"
    );
    assert_ne!(
        field_of(&facade, ACTION, "instanceActionId", &waiting, "statusEnumId"),
        "IasWaiting"
    );

    // a deployment's prepare and deploy are its deploy step's: a caller with authorization on is refused
    login(&fleet.vault.maker.user);
    for service in ["prepare#GitDeployment", "deploy#GitDeployment"] {
        let (refused, _) = call(
            service,
            &[
                ("deploymentId", "100000"),
                ("componentName", "site"),
                ("repositoryUrl", SITE_URL),
            ],
        );
        assert!(refused.contains("deploy step's"), "{service}: {refused}");
    }
}

/// A rollout paused in Monitor (pause-rollout, the environment's `rolloutPausedDate`, M4) stops a roll before its next
/// instance: the ones before it changed, the rest did not.
#[test]
fn a_paused_rollout_stops_before_the_next_instance() {
    let (ctx, facade) = super::setup();
    let _guard = ctx.enter();
    let fleet = fleet(&facade, "pause", FakeEngine::docker());
    login(&fleet.vault.maker.user);
    let registry = lease::registry(ctx.factory());
    let first = app(&facade, &fleet, &registry, "app-pause-1", "");
    let second = app(&facade, &fleet, &registry, "app-pause-2", "");
    gated(&facade, &fleet);
    set_row(
        &facade,
        "moqui.server.instance.InstanceEnvironment",
        "environmentId",
        &fleet.environment,
        &[("rolloutPausedDate", "now")],
    );
    let link = FakeLink::default();
    let refused = deploy::run(
        &facade,
        &registry,
        &link,
        &component_deploy(&fleet, COMMIT_PASSED),
        None,
    )
    .unwrap_err();
    assert!(refused.message().contains("rolloutPaused"), "{refused}");
    assert_eq!(
        link.changes(),
        [format!("deploy {first} {COMMIT_PASSED} drained=false rollback=false")]
    );
    assert!(!actions_of(&facade, &second)
        .iter()
        .any(|(action, _)| action == "IatDeploy"));
}
