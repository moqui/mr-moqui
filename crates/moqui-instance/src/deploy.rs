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

//! Deploys to managed instances (plans/archive/Vault-Instances.md §4.6, M3; Dogfood §8.6, DG10), in the steps of the plan:
//!
//! 1. **Trigger** ([`Change`]): a `GitDeployment` of an environment that names this `InstanceEnvironment`
//!    (crates/moqui-git calls `prepare#GitDeployment` once its pipeline passed and `deploy#GitDeployment` once it is
//!    approved), or one made by hand on the instance or environment page: a new image, a component's commit, or the
//!    rollback of a deploy.
//! 2. **Gate** ([`gate`]): without a passed run of the environment's required pipeline for the commit, the deploy is
//!    refused; the manager checks the run itself. A rollback goes to a version the environment ran, and is not gated.
//! 3. **Approval**: what the deploy opens (a host's client key, a registry login, the secret files of a replaced
//!    container, a database's admin login for its backup) is leased; approving is opening the lease, unless the
//!    environment has machine access ([`crate::actions::open_authority`]). The lease's operation names the deploy and
//!    where each item goes, and a lease of another operation is refused.
//! 4. **Plan** ([`plan`]): for each instance, the image digest and component commits before and after, and the schema
//!    hash before.
//! 5. **Back up first**: a change that touches a database runs the database kind's backup service first (an mr-moqui
//!    instance's own database, or a database instance whose image changes); a failed backup refuses the deploy before
//!    anything changes. A database host not managed here keeps its own backups, and the report says so.
//! 6. **Roll** ([`run`]): one instance at a time. A cluster member is drained over the link, changed, waited for until it
//!    reports healthy and unsealed (a container that is not mr-moqui: running and healthy), and undrained; a single
//!    instance is replaced in place. A new image is a new container with the same volumes, settings and secret files
//!    (the old one is put back when the new one does not start or come up); a component is Component-Deploy's update
//!    over the link, as the link's caller, with no attestation key on the instance. The first instance that fails stops
//!    the roll.
//! 7. **Mark**: each changed instance gets an annotation (service, instance, version, revision) in Monitor; one that is
//!    not posted is logged and reported, and the deploy goes on.
//! 8. **Rollback**: to the image digest and component commits a deploy action recorded before it, refused when the
//!    instance's schema hash is not the one it had before that deploy (a schema change is not rolled back).
//!
//! Each instance's change is an `InstanceAction` with its versions and schema hash before and after, its lease (or
//! machine access), its `GitDeployment` and its report. Postgres, MySQL and OpenSearch take a minor version as a
//! container replacement; a major version upgrade is the kind's own procedure, not built here.
//!
//! The link's calls go through [`Link`], the manager link's `call#InstanceCommand` ([`ServiceLink`]); the tests give a
//! fake of it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value, ValueMap};
use moqui_vault_server::lease::{Ask, LeaseRegistry};
use serde_json::{json, Value as Json};

use crate::actions::{self, Opened, Requested};
use crate::common::{
    connect_host, fail, field, find, host_needs_key, one, rows, ACTION, APP_INSTANCE, ENVIRONMENT, IMAGE,
    INSTANCE_DEPENDENCY, INSTANCE_SECRET, KIND, SERVICE_PACKAGE,
};
use crate::plan::{self, Destination};

const PIPELINE_RUN: &str = "moqui.resource.GitPipelineRun";
const DEPLOYMENT: &str = "moqui.resource.GitDeployment";
const CI_ENVIRONMENT: &str = "moqui.resource.GitCiEnvironment";
const LINK_COMMAND: &str = "org.moqui.dev.InstanceLinkServices.call#InstanceCommand";
const ANNOTATION: &str = "org.moqui.report.ReportServices.create#Annotation";
/// The attestation format of moqui-git's `ci::attest`, which the link's deploy carries unsigned.
const ATTESTATION_KIND: &str = "moqui-deploy-attestation";
const ATTESTATION_VERSION: u32 = 2;
/// How often a deploy asks whether a changed instance is up.
const POLL: Duration = Duration::from_secs(2);
/// How long a container is given to stop before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(30);
/// The longest operation a lease names.
const MAX_OPERATION: usize = 255;

// ----- the link -----

/// What a managed instance's link says of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkStatus {
    pub seal_state: String,
    pub health: String,
    pub schema_hash: String,
}

impl LinkStatus {
    /// Healthy and unsealed: up, whether drained or not.
    pub fn ready(&self) -> bool {
        self.seal_state == "unsealed" && matches!(self.health.as_str(), "ok" | "draining")
    }
}

/// The calls a deploy makes over the manager link (§4.4): status, drain and undrain, and Component-Deploy's update.
pub trait Link: Send + Sync {
    fn status(&self, instance_id: &str) -> Result<LinkStatus, LibError>;
    fn drain(&self, instance_id: &str, drained: bool) -> Result<(), LibError>;
    /// The deploy of the commit `attestation` names, unsigned: the link's pinned keys name its caller (§4.6).
    fn deploy_component(&self, instance_id: &str, attestation: &str) -> Result<String, LibError>;
}

/// The manager link of this server: status, drain and undrain through `call#InstanceCommand`, which checks the caller and
/// the instance's environment and sends only to a link it accepts; the unsigned deploy through the link server's Rust
/// path ([`crate::link::link_deploy`]), never a service's parameter.
#[derive(Debug, Clone, Copy, Default)]
pub struct ServiceLink;

impl ServiceLink {
    fn command(instance_id: &str, command: &str) -> Result<BTreeMap<String, Value>, LibError> {
        let parameters: BTreeMap<String, Value> = [
            ("appInstanceId".to_string(), Value::string(instance_id)),
            ("command".to_string(), Value::string(command)),
        ]
        .into();
        crate::lifecycle::call(LINK_COMMAND, &parameters)
    }
}

impl Link for ServiceLink {
    fn status(&self, instance_id: &str) -> Result<LinkStatus, LibError> {
        let out = Self::command(instance_id, "status")?;
        let text = |name: &str| out.get(name).map(to_plain_string).unwrap_or_default();
        Ok(LinkStatus {
            seal_state: text("sealState"),
            health: text("health"),
            schema_hash: text("schemaHash"),
        })
    }

    fn drain(&self, instance_id: &str, drained: bool) -> Result<(), LibError> {
        Self::command(instance_id, if drained { "drain" } else { "undrain" }).map(|_| ())
    }

    fn deploy_component(&self, instance_id: &str, attestation: &str) -> Result<String, LibError> {
        crate::link::link_deploy(instance_id, attestation)
    }
}

// ----- the trigger -----

/// A component's commit, deployed over the link.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Component {
    pub name: String,
    pub repository_url: String,
    /// The branch, a full ref (`refs/heads/main`).
    pub ref_name: String,
    pub commit_id: String,
    pub data_types: Vec<String>,
}

/// What a deploy changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A new image of the instances' kind: each takes a new container from it.
    Image { image_id: String },
    /// A component's commit on the environment's mr-moqui instances.
    Component(Component),
    /// Back to what the deploy action `action_id` recorded before it.
    Rollback { action_id: String },
}

/// A deploy asked for.
#[derive(Debug, Clone)]
pub struct Request {
    pub environment_id: String,
    pub change: Change,
    /// The commit the gate checks: a component's, or the revision an image was built from.
    pub commit_id: String,
    /// The repository whose pipeline ran, when known.
    pub repository_id: String,
    /// One instance, or one cluster, of the environment; all of the change's kind when both are empty.
    pub instance_id: String,
    pub cluster_id: String,
    pub git_deployment_id: String,
    /// The pipeline whose passed run gates it when the environment names none: a deployment's own.
    pub fallback_pipeline: String,
    /// The source server's URL, for the instances' log.
    pub source: String,
}

impl Request {
    pub fn new(environment_id: &str, change: Change) -> Request {
        Request {
            environment_id: environment_id.to_string(),
            change,
            commit_id: String::new(),
            repository_id: String::new(),
            instance_id: String::new(),
            cluster_id: String::new(),
            git_deployment_id: String::new(),
            fallback_pipeline: String::new(),
            source: String::new(),
        }
    }

    fn verb(&self) -> &'static str {
        match self.change {
            Change::Rollback { .. } => "rollback",
            _ => "deploy",
        }
    }

    /// What the lease's operation says is deployed.
    fn what(&self) -> String {
        if !self.git_deployment_id.is_empty() {
            return format!("GitDeployment {}", self.git_deployment_id);
        }
        match &self.change {
            Change::Image { image_id } => format!("image {image_id}"),
            Change::Component(component) => format!("component {} at {}", component.name, short(&component.commit_id)),
            Change::Rollback { action_id } => format!("to before InstanceAction {action_id}"),
        }
    }
}

fn short(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

// ----- the gate -----

/// The gate (§4.6, step 2): the id of a passed run of the environment's required pipeline (or the request's fallback,
/// a deployment's own) for what is deployed, never for a commit a caller names beside it:
///
/// - a component: of the repository a CI environment linked to this environment deploys (its `repositoryId`), at the
///   component's commit, and from that CI environment's clone URL (a deploy by hand takes its URL from it);
/// - an image: of the source repository and revision recorded on its `InstanceImage` by the pipeline that built it.
///
/// A rollback is not gated: it goes back to a version the environment ran.
pub fn gate(facade: &EntityFacade, environment: &EntityValue, request: &Request) -> Result<String, LibError> {
    let environment_id = field(environment, "environmentId");
    if matches!(request.change, Change::Rollback { .. }) {
        return Ok(String::new());
    }
    let pipeline = Some(field(environment, "requiredPipeline"))
        .filter(|pipeline| !pipeline.is_empty())
        .unwrap_or_else(|| request.fallback_pipeline.clone());
    if pipeline.is_empty() {
        return Err(fail(format!(
            "InstanceEnvironment {environment_id} names no required pipeline: it deploys only a commit that pipeline \
             passed (reasonCode noGate)"
        )));
    }
    let (repository, commit) = match &request.change {
        Change::Rollback { .. } => return Ok(String::new()),
        Change::Image { image_id } => {
            let image = one(facade, IMAGE, "instanceImageId", image_id)?
                .ok_or_else(|| fail(format!("No InstanceImage {image_id}")))?;
            let (repository, revision) = (field(&image, "sourceRepositoryId"), field(&image, "sourceRevision"));
            if repository.is_empty() || revision.is_empty() {
                return Err(fail(format!(
                    "InstanceImage {image_id} records no source repository and revision (written by the pipeline that built it): its passed run cannot be checked (reasonCode noRevision)"
                )));
            }
            (repository, revision)
        }
        Change::Component(component) => {
            if request.repository_id.is_empty() {
                return Err(fail(format!(
                    "A component deploy to {environment_id} names its repository (repositoryId), whose pipeline must have passed for the commit (reasonCode noRepository)"
                )));
            }
            let linked = find(facade, CI_ENVIRONMENT)?
                .eq("instanceEnvironmentId", Value::string(&environment_id))
                .eq("repositoryId", Value::string(&request.repository_id))
                .list()?
                .into_values();
            let Some(ci) = linked.first() else {
                return Err(fail(format!(
                    "No CI environment of repository {} deploys to InstanceEnvironment {environment_id} (reasonCode notLinked)",
                    request.repository_id
                )));
            };
            let clone_url = field(ci, "cloneUrl");
            if clone_url.is_empty() && request.git_deployment_id.is_empty() {
                return Err(fail(format!(
                    "The CI environment of repository {} names no cloneUrl: a deploy by hand takes the repository's URL                      from it (reasonCode noCloneUrl)",
                    request.repository_id
                )));
            }
            if !clone_url.is_empty() && clone_url != component.repository_url {
                return Err(fail(format!(
                    "The component's repository URL is not its CI environment's cloneUrl {clone_url} (reasonCode urlMismatch)"
                )));
            }
            (request.repository_id.clone(), component.commit_id.clone())
        }
    };
    if commit.is_empty() {
        return Err(fail(format!(
            "A deploy to {environment_id} names no commit its pipeline {pipeline} passed for (reasonCode noCommit)"
        )));
    }
    let runs = find(facade, PIPELINE_RUN)?
        .eq("pipelineName", Value::string(&pipeline))
        .eq("repositoryId", Value::string(&repository))
        .eq("commitId", Value::string(&commit))
        .eq("statusId", Value::string("success"));
    match runs.list()?.iter().next() {
        Some(run) => Ok(field(run, "pipelineRunId")),
        None => Err(fail(format!(
            "No passed run of pipeline {pipeline} of repository {repository} for commit {}: InstanceEnvironment \
             {environment_id} deploys only what it passed (reasonCode notPassed)",
            short(&commit)
        ))),
    }
}

// ----- the plan -----

/// One instance's change.
#[derive(Debug, Clone)]
pub struct Target {
    pub instance: EntityValue,
    pub kind: EntityValue,
    pub cluster_id: String,
    /// Its image digest before.
    pub image_before: String,
    /// The image its new container is made from, when its container is replaced.
    pub image_after: Option<String>,
    /// The `InstanceImage` of the new image (its registry login), when there is one.
    pub image_row: Option<EntityValue>,
    /// Its component commits before and after (name to `{commit, repositoryUrl, refName}`).
    pub components_before: BTreeMap<String, Json>,
    pub components_after: BTreeMap<String, Json>,
    /// The component updates it takes over the link, in order.
    pub component_deploys: Vec<Component>,
    /// The schema hash its link last reported.
    pub schema_before: String,
    /// A cluster member: drained over the link while it changes.
    pub drain: bool,
    /// Its image goes back to a recorded digest: pulled only when the host does not have it.
    pub rollback: bool,
}

impl Target {
    pub fn id(&self) -> String {
        field(&self.instance, "appInstanceId")
    }

    fn kind_id(&self) -> String {
        field(&self.kind, "instanceKindId")
    }

    fn name(&self) -> String {
        field(&self.instance, "instanceName")
    }
}

/// A backup a deploy runs first: of a database server instance, one database or all.
#[derive(Debug, Clone)]
pub struct Backup {
    pub server: EntityValue,
    pub database: Option<String>,
    pub service: String,
}

/// A deploy's plan.
#[derive(Debug, Clone)]
pub struct DeployPlan {
    pub environment_id: String,
    pub vault_id: String,
    pub machine_access: bool,
    pub lease_limit: Option<Duration>,
    /// The instances, in the order they roll: cluster by cluster.
    pub targets: Vec<Target>,
    pub backups: Vec<Backup>,
    /// What the report says beside: a database that is not backed up here, and why.
    pub notes: Vec<String>,
    /// The vault items it opens, by binding: what its lease must name.
    pub bindings: Vec<String>,
    pub destinations: Vec<Destination>,
    /// The operation its lease names.
    pub operation: String,
    pub rollback_of: Option<String>,
}

/// An instance's component commits as recorded (`componentCommits`, JSON), name to `{commit, repositoryUrl, refName}`.
fn components_of(instance: &EntityValue) -> BTreeMap<String, Json> {
    serde_json::from_str::<BTreeMap<String, Json>>(&field(instance, "componentCommits")).unwrap_or_default()
}

fn commit_of(entry: &Json) -> &str {
    entry["commit"].as_str().unwrap_or_default()
}

/// The instances of `environment_id` that `request` changes, made and wanted running, in the order they roll.
fn instances_of(facade: &EntityFacade, request: &Request, kind_id: &str) -> Result<Vec<EntityValue>, LibError> {
    let mut found: Vec<EntityValue> = find(facade, APP_INSTANCE)?
        .eq("environmentId", Value::string(&request.environment_id))
        .eq("instanceKindId", Value::string(kind_id))
        .list()?
        .into_values()
        .into_iter()
        .filter(|row| plan::is_made(row) && matches!(field(row, "desiredStateEnumId").as_str(), "" | "IdsRunning"))
        .filter(|row| request.instance_id.is_empty() || field(row, "appInstanceId") == request.instance_id)
        .filter(|row| request.cluster_id.is_empty() || field(row, "clusterId") == request.cluster_id)
        .collect();
    found.sort_by_key(|row| {
        (
            field(row, "clusterId"),
            field(row, "instanceName"),
            field(row, "appInstanceId"),
        )
    });
    Ok(found)
}

/// Whether `instance` is one of several made members of its cluster.
fn clustered(facade: &EntityFacade, instance: &EntityValue) -> Result<bool, LibError> {
    let cluster = field(instance, "clusterId");
    if cluster.is_empty() {
        return Ok(false);
    }
    let members = rows(facade, APP_INSTANCE, "clusterId", &cluster)?
        .iter()
        .filter(|row| plan::is_made(row))
        .count();
    Ok(members > 1)
}

/// The plan of `request` (§4.6, step 4): its instances with their versions before and after, its backups, and the lease
/// it needs. Refused when there is nothing to change, an image is not of the instances' kind, or a rollback crosses a
/// schema change.
pub fn plan(facade: &EntityFacade, link: &dyn Link, request: &Request) -> Result<DeployPlan, LibError> {
    let environment = one(facade, ENVIRONMENT, "environmentId", &request.environment_id)?
        .ok_or_else(|| fail(format!("No InstanceEnvironment {}", request.environment_id)))?;
    let vault_id = field(&environment, "vaultId");
    if vault_id.is_empty() {
        return Err(fail(format!(
            "InstanceEnvironment {} names no operational vault",
            request.environment_id
        )));
    }
    let mut targets = Vec::new();
    let mut rollback_of = None;
    match &request.change {
        Change::Image { image_id } => {
            let image = one(facade, IMAGE, "instanceImageId", image_id)?
                .ok_or_else(|| fail(format!("No InstanceImage {image_id}")))?;
            let kind_id = field(&image, "instanceKindId");
            let named = field(&image, "vaultId");
            if !named.is_empty() && named != vault_id {
                return Err(fail(format!(
                    "InstanceImage {image_id} keeps its items in vault {named}, not the environment's vault {vault_id}"
                )));
            }
            if field(&image, "registryLogin") == "Y" {
                plan::check_registry(&image)?;
            }
            for row in instances_of(facade, request, &kind_id)? {
                let mut target = target_of(facade, link, row)?;
                refuse_major(facade, &target, &image)?;
                if kind_id == "mr-moqui" && field(&target.instance, "linkPairedDate").is_empty() {
                    return Err(fail(format!(
                        "AppInstance {} has not paired with this manager: a deploy waits for its link to report it \
                         healthy and unsealed, so pair it first",
                        target.id()
                    )));
                }
                target.image_after = Some(field(&image, "imageName"));
                target.image_row = Some(image.clone());
                targets.push(target);
            }
        }
        Change::Component(component) => {
            for row in instances_of(facade, request, "mr-moqui")? {
                let mut target = target_of(facade, link, row)?;
                if !component.name.is_empty() {
                    target.components_after.insert(
                        component.name.clone(),
                        json!({ "commit": component.commit_id, "repositoryUrl": component.repository_url,
                                "refName": component.ref_name }),
                    );
                }
                target.component_deploys.push(component.clone());
                targets.push(target);
            }
        }
        Change::Rollback { action_id } => {
            let action = one(facade, ACTION, "instanceActionId", action_id)?
                .ok_or_else(|| fail(format!("No InstanceAction {action_id}")))?;
            if !matches!(field(&action, "actionEnumId").as_str(), "IatDeploy" | "IatRollback")
                || field(&action, "statusEnumId") != actions::DONE
            {
                return Err(fail(format!(
                    "InstanceAction {action_id} is not a deploy that was done: only a done deploy is rolled back"
                )));
            }
            let instance = plan::instance(facade, &field(&action, "appInstanceId"))?;
            if field(&instance, "environmentId") != request.environment_id {
                return Err(fail(format!(
                    "InstanceAction {action_id} is of another environment than {}",
                    request.environment_id
                )));
            }
            let mut target = target_of(facade, link, instance)?;
            let before = field(&action, "schemaHashBefore");
            let after = field(&action, "schemaHashAfter");
            // an mr-moqui instance's schema is known only by its link: unknown then or now is refused, never assumed
            let unknown = target.kind_id() == "mr-moqui"
                && (before.is_empty() || after.is_empty() || target.schema_before.is_empty());
            if unknown || after != before || target.schema_before != before {
                return Err(fail(format!(
                    "AppInstance {}'s schema changed since before InstanceAction {action_id}, or is not known (schema \
                     {:?} then, {:?} now): a rollback across a schema change is refused; restore its backup instead (reasonCode \
                     schemaChanged)",
                    target.id(),
                    short(&before),
                    short(&target.schema_before)
                )));
            }
            let image_before = field(&action, "versionBefore");
            if !image_before.is_empty() && image_before != target.image_before {
                target.image_after = Some(image_before);
                // the image it ran before, whose registry login a pull of that digest takes
                let image_row = one(
                    facade,
                    IMAGE,
                    "instanceImageId",
                    &field(&action, "instanceImageIdBefore"),
                )?;
                if let Some(image_row) = image_row.as_ref().filter(|image| field(image, "registryLogin") == "Y") {
                    plan::check_registry(image_row)?;
                }
                target.image_row = image_row;
                target.rollback = true;
            }
            let recorded: BTreeMap<String, Json> =
                serde_json::from_str(&field(&action, "componentCommitsBefore")).unwrap_or_default();
            for (name, now) in &target.components_before {
                if !recorded.contains_key(name) {
                    return Err(fail(format!(
                        "Component {name} on AppInstance {} came after InstanceAction {action_id}'s before: a rollback \
                         does not remove a component",
                        target.id()
                    )));
                }
                let then = &recorded[name];
                if commit_of(then) != commit_of(now) {
                    target.component_deploys.push(Component {
                        name: name.clone(),
                        repository_url: then["repositoryUrl"].as_str().unwrap_or_default().to_string(),
                        ref_name: then["refName"].as_str().unwrap_or_default().to_string(),
                        commit_id: commit_of(then).to_string(),
                        data_types: Vec::new(),
                    });
                }
            }
            target.components_after = recorded;
            if target.image_after.is_none() && target.component_deploys.is_empty() {
                return Err(fail(format!(
                    "AppInstance {} is at the version InstanceAction {action_id} recorded before it: nothing to roll back",
                    target.id()
                )));
            }
            rollback_of = Some(action_id.clone());
            targets.push(target);
        }
    }
    if targets.is_empty() {
        return Err(fail(format!(
            "InstanceEnvironment {} has no made instance that {} changes",
            request.environment_id,
            request.what()
        )));
    }
    for target in &mut targets {
        target.drain = target.kind_id() == "mr-moqui" && clustered(facade, &target.instance)?;
    }
    let (backups, notes) = backups_of(facade, &targets)?;
    let mut destinations: Vec<Destination> = Vec::new();
    let mut add = |destination: Destination| {
        if !destinations.iter().any(|known| known.binding == destination.binding) {
            destinations.push(destination);
        }
    };
    for target in targets.iter().filter(|target| target.image_after.is_some()) {
        let host = crate::lifecycle::host_of(facade, &target.instance)?;
        if host_needs_key(&host) {
            add(plan::host_destination(&host));
        }
        if let Some(image) = target
            .image_row
            .as_ref()
            .filter(|image| field(image, "registryLogin") == "Y")
        {
            add(Destination {
                binding: format!("InstanceImage:{}:registry", field(image, "instanceImageId")),
                to: plan::registry_name(&field(image, "registryLocation")),
            });
        }
        for secret in rows(facade, INSTANCE_SECRET, "appInstanceId", &target.id())? {
            if !field(&secret, "fileName").is_empty() {
                add(Destination {
                    binding: field(&secret, "bindingKey"),
                    to: format!("container {}", target.name()),
                });
            }
        }
    }
    for backup in &backups {
        let server = &backup.server;
        let kind = one(facade, KIND, "instanceKindId", &field(server, "instanceKindId"))?
            .ok_or_else(|| fail(format!("AppInstance {} has no kind", field(server, "appInstanceId"))))?;
        add(Destination {
            binding: format!("AppInstance:{}:admin", field(server, "appInstanceId")),
            to: format!("{}:{}", field(server, "instanceName"), field(&kind, "containerPort")),
        });
        let host = crate::lifecycle::host_of(facade, server)?;
        if host_needs_key(&host) {
            add(plan::host_destination(&host));
        }
    }
    let operation = operation(request, &destinations)?;
    Ok(DeployPlan {
        environment_id: request.environment_id.clone(),
        vault_id,
        machine_access: field(&environment, "machineAccess") == "Y",
        lease_limit: plan::lease_limit(&environment),
        targets,
        backups,
        notes,
        bindings: destinations
            .iter()
            .map(|destination| destination.binding.clone())
            .collect(),
        destinations,
        operation,
        rollback_of,
    })
}

/// Refuses a new image of a database or search kind whose tag's major version is not the running one's: a major
/// upgrade is the kind's own procedure (§4.6), not a container replacement.
fn refuse_major(facade: &EntityFacade, target: &Target, image: &EntityValue) -> Result<(), LibError> {
    if !matches!(target.kind_id().as_str(), "postgres" | "mysql" | "opensearch") {
        return Ok(());
    }
    let major = |name: &str| -> String {
        let tag = match name.rsplit_once(':') {
            Some((_, tag)) if !tag.contains('/') => tag,
            _ => "",
        };
        tag.split(['.', '-']).next().unwrap_or_default().to_string()
    };
    let running = one(
        facade,
        IMAGE,
        "instanceImageId",
        &field(&target.instance, "instanceImageId"),
    )?
    .map(|row| field(&row, "imageName"))
    .unwrap_or_default();
    let (now, next) = (major(&running), major(&field(image, "imageName")));
    if now.is_empty() || next.is_empty() || now != next {
        return Err(fail(format!(
            "AppInstance {} runs {running} and {} is another major version (or names none): a major upgrade is the \
             kind's own procedure, not a deploy (reasonCode majorUpgrade)",
            target.id(),
            field(image, "imageName")
        )));
    }
    Ok(())
}

fn target_of(facade: &EntityFacade, link: &dyn Link, instance: EntityValue) -> Result<Target, LibError> {
    let kind_id = field(&instance, "instanceKindId");
    let kind = one(facade, KIND, "instanceKindId", &kind_id)?.ok_or_else(|| {
        fail(format!(
            "AppInstance {} has no InstanceKind {kind_id}",
            field(&instance, "appInstanceId")
        ))
    })?;
    let components = components_of(&instance);
    Ok(Target {
        kind,
        cluster_id: field(&instance, "clusterId"),
        image_before: field(&instance, "imageDigest"),
        image_after: None,
        image_row: None,
        components_after: components.clone(),
        components_before: components,
        component_deploys: Vec::new(),
        // an mr-moqui instance's schema as its link reports it now; unknown (empty) when it does not answer
        schema_before: if kind_id == "mr-moqui" {
            link.status(&field(&instance, "appInstanceId"))
                .map(|status| status.schema_hash)
                .unwrap_or_default()
        } else {
            String::new()
        },
        drain: false,
        rollback: false,
        instance,
    })
}

/// The backups a deploy runs first (§4.6, step 5), once each: a database instance whose container is replaced (all its
/// databases), and the database of each mr-moqui instance it changes, on a database instance. A database host not
/// managed here, and an embedded database, are not backed up here: a note says so.
fn backups_of(facade: &EntityFacade, targets: &[Target]) -> Result<(Vec<Backup>, Vec<String>), LibError> {
    let mut backups: Vec<Backup> = Vec::new();
    let mut notes = Vec::new();
    let mut add = |server: &EntityValue, database: Option<String>, service: String| {
        let id = field(server, "appInstanceId");
        if !backups
            .iter()
            .any(|known| field(&known.server, "appInstanceId") == id && known.database == database)
        {
            backups.push(Backup {
                server: server.clone(),
                database,
                service,
            });
        }
    };
    for target in targets {
        let service = field(&target.kind, "backupService");
        if !service.is_empty() {
            if target.image_after.is_some() {
                add(&target.instance, None, service);
            }
            continue;
        }
        if target.kind_id() != "mr-moqui" {
            continue;
        }
        let id = target.id();
        let dependency = rows(facade, INSTANCE_DEPENDENCY, "appInstanceId", &id)?
            .into_iter()
            .find(|dependency| field(dependency, "dependencyRole") == "database");
        let Some(dependency) = dependency else {
            notes.push(format!(
                "AppInstance {id}'s database is embedded: it is in its own volume, which a deploy keeps, and is backed \
                 up by its own schedule, not here"
            ));
            continue;
        };
        let server_id = field(&dependency, "dependsOnInstanceId");
        if server_id.is_empty() {
            notes.push(format!(
                "AppInstance {id}'s database is on DatabaseHost {}, which keeps its own backups: not backed up here",
                field(&dependency, "databaseHostId")
            ));
            continue;
        }
        let server = plan::instance(facade, &server_id)?;
        let server_kind = one(facade, KIND, "instanceKindId", &field(&server, "instanceKindId"))?
            .ok_or_else(|| fail(format!("AppInstance {server_id} has no InstanceKind")))?;
        let service = field(&server_kind, "backupService");
        if service.is_empty() {
            return Err(fail(format!(
                "AppInstance {id}'s database, AppInstance {server_id}, has no backup service: a deploy that touches a \
                 database backs it up first"
            )));
        }
        add(&server, Some(field(&target.instance, "databaseName")), service);
    }
    Ok((backups, notes))
}

/// The operation a deploy's lease names: the verb, what, the environment and where each item goes; refused past what a
/// lease holds.
fn operation(request: &Request, destinations: &[Destination]) -> Result<String, LibError> {
    let mut text = format!(
        "{} {} to InstanceEnvironment {}",
        request.verb(),
        request.what(),
        request.environment_id
    );
    if !destinations.is_empty() {
        let to: Vec<String> = destinations
            .iter()
            .map(|destination| format!("{} to {}", destination.binding, destination.to))
            .collect();
        text = format!("{text}; {}", to.join(", "));
    }
    if text.len() > MAX_OPERATION || text.chars().any(char::is_control) {
        return Err(fail(format!(
            "The lease of {} {} would name {} characters, more than a lease holds ({MAX_OPERATION}): deploy to one \
             cluster or instance at a time",
            request.verb(),
            request.what(),
            text.len()
        )));
    }
    Ok(text)
}

/// The preview of a deploy: its instances with their versions before and after, its backups and notes, and the lease it
/// needs. No value of any kind.
pub fn preview(plan: &DeployPlan) -> ValueMap {
    fn text(given: impl AsRef<str>) -> Value {
        Value::string(given.as_ref())
    }
    let instances = plan
        .targets
        .iter()
        .map(|target| {
            let mut map = ValueMap::default();
            map.insert("appInstanceId".to_string(), text(target.id()));
            map.insert("name".to_string(), text(target.name()));
            map.insert("clusterId".to_string(), text(&target.cluster_id));
            map.insert("imageBefore".to_string(), text(&target.image_before));
            map.insert(
                "imageAfter".to_string(),
                text(target.image_after.as_deref().unwrap_or(&target.image_before)),
            );
            map.insert(
                "componentsBefore".to_string(),
                text(json!(target.components_before).to_string()),
            );
            map.insert(
                "componentsAfter".to_string(),
                text(json!(target.components_after).to_string()),
            );
            map.insert("schemaBefore".to_string(), text(&target.schema_before));
            map.insert("drained".to_string(), Value::Bool(target.drain));
            map.insert("replaced".to_string(), Value::Bool(target.image_after.is_some()));
            Value::Map(map)
        })
        .collect();
    let backups = plan
        .backups
        .iter()
        .map(|backup| {
            text(format!(
                "{} of AppInstance {}",
                backup.database.as_deref().unwrap_or("every database"),
                field(&backup.server, "appInstanceId")
            ))
        })
        .collect();
    let mut preview = ValueMap::default();
    preview.insert("environmentId".to_string(), text(&plan.environment_id));
    preview.insert("instances".to_string(), Value::List(instances));
    preview.insert("backups".to_string(), Value::List(backups));
    preview.insert("notes".to_string(), Value::List(plan.notes.iter().map(text).collect()));
    preview.insert(
        "leaseBindings".to_string(),
        Value::List(plan.bindings.iter().map(text).collect()),
    );
    preview.insert(
        "destinations".to_string(),
        Value::List(
            plan.destinations
                .iter()
                .map(|destination| text(format!("{} to {}", destination.binding, destination.to)))
                .collect(),
        ),
    );
    preview.insert("machineAccess".to_string(), Value::Bool(plan.machine_access));
    preview.insert("operation".to_string(), text(&plan.operation));
    preview
}

// ----- the run -----

/// What a deploy did: an action per instance it changed, and its report.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub action_ids: Vec<String>,
    pub instance_ids: Vec<String>,
    pub report: Vec<String>,
}

/// Runs a deploy (see the module doc), under the lease `lease_id` or the environment's machine access when it opens vault
/// items. A refusal is an action row on each of its instances, as M1's are.
pub fn run(
    facade: &EntityFacade,
    registry: &LeaseRegistry,
    link: &dyn Link,
    request: &Request,
    lease_id: Option<&str>,
) -> Result<Outcome, LibError> {
    let plan = plan(facade, link, request)?;
    let environment = one(facade, ENVIRONMENT, "environmentId", &plan.environment_id)?
        .ok_or_else(|| fail(format!("No InstanceEnvironment {}", plan.environment_id)))?;
    actions::check_caller(&environment)?;
    let refuse = |err: LibError| -> Result<Outcome, LibError> {
        for target in &plan.targets {
            let id = actions::record(
                facade,
                &target.id(),
                request.verb(),
                actions::REFUSED,
                lease_id,
                false,
                err.message(),
            )?;
            set_fields(
                facade,
                &id,
                &[("gitDeploymentId", Value::string(&request.git_deployment_id))],
            )?;
        }
        Err(err)
    };
    if let Err(err) = gate(facade, &environment, request) {
        return refuse(err);
    }
    let opened = match actions::open_authority(
        facade,
        registry,
        &plan.environment_id,
        &plan.vault_id,
        plan.machine_access,
        &plan.bindings,
        &plan.operation,
        lease_id,
    ) {
        Ok(opened) => opened,
        Err(err) => return refuse(err),
    };
    let mut outcome = Outcome {
        report: plan.notes.clone(),
        ..Outcome::default()
    };
    let ran = roll(facade, link, &plan, request, &opened, &mut outcome);
    actions::end_lease(facade, registry, &opened);
    ran.map(|()| outcome)
}

/// Runs `body` under the deploy's lease, when it has one: only the steps that read a leased value (a replaced
/// container's secret files and its image's pull, a host reached with its client key, a backup's admin login); the
/// annotations and the rows are written outside it.
fn under<T>(opened: &Opened, body: impl FnOnce() -> T) -> T {
    match &opened.lease_id {
        Some(lease_id) => moqui_vault_server::lease::run_under(lease_id, body),
        None => body(),
    }
}

/// The backups, then each instance in turn; the first failure stops the roll.
fn roll(
    facade: &EntityFacade,
    link: &dyn Link,
    plan: &DeployPlan,
    request: &Request,
    opened: &Opened,
    outcome: &mut Outcome,
) -> Result<(), LibError> {
    for backup in &plan.backups {
        run_backup(facade, backup, request, opened, outcome)?;
    }
    for (at, target) in plan.targets.iter().enumerate() {
        // between instances: a rollout paused in Monitor (pause-rollout, §4.8) stops here, the ones before it changed
        if at > 0 && crate::monitor::rollout_paused(facade, &plan.environment_id)? {
            let line = format!(
                "the rollout of InstanceEnvironment {} is paused: stopped before AppInstance {} (reasonCode \
                 rolloutPaused)",
                plan.environment_id,
                target.id()
            );
            outcome.report.push(line.clone());
            return Err(fail(line));
        }
        change(facade, link, plan, target, request, opened, outcome)?;
    }
    Ok(())
}

/// The rollback of an instance's last deploy that was done: back to the versions it recorded before it.
pub fn rollback_request(facade: &EntityFacade, instance_id: &str) -> Result<Request, LibError> {
    let instance = plan::instance(facade, instance_id)?;
    let last = find(facade, ACTION)?
        .eq("appInstanceId", Value::string(instance_id))
        .eq("actionEnumId", Value::string("IatDeploy"))
        .eq("statusEnumId", Value::string(actions::DONE))
        .order_by("startDate", true)
        .order_by("instanceActionId", true)
        .limit(1)
        .list()?
        .iter()
        .next()
        .map(|row| field(row, "instanceActionId"))
        .ok_or_else(|| fail(format!("AppInstance {instance_id} has no deploy to roll back")))?;
    let mut request = Request::new(&field(&instance, "environmentId"), Change::Rollback { action_id: last });
    request.instance_id = instance_id.to_string();
    Ok(request)
}

/// The `rollback` instance action (`actions::run`): the rollback of the instance's last deploy, over the manager link,
/// under `lease_id` or the environment's machine access.
pub fn rollback_last(
    facade: &EntityFacade,
    registry: &LeaseRegistry,
    instance_id: &str,
    lease_id: Option<&str>,
) -> Result<crate::provision::Outcome, LibError> {
    let request = rollback_request(facade, instance_id)?;
    let outcome = run(facade, registry, &ServiceLink, &request, lease_id)?;
    Ok(crate::provision::Outcome {
        action_id: outcome.action_ids.last().cloned().unwrap_or_default(),
        report: outcome.report,
    })
}

/// Sets fields of an action row.
fn set_fields(facade: &EntityFacade, action_id: &str, fields: &[(&str, Value)]) -> Result<(), LibError> {
    let Some(mut row) = one(facade, ACTION, "instanceActionId", action_id)? else {
        return Err(fail(format!("No InstanceAction {action_id}")));
    };
    for (name, value) in fields {
        let empty = matches!(value, Value::String(text) if text.is_empty());
        row.set(*name, if empty { Value::Null } else { value.clone() });
    }
    row.update(facade)?;
    Ok(())
}

/// A backup before the change, as an action of the database instance; one that fails refuses the deploy before anything
/// changed.
fn run_backup(
    facade: &EntityFacade,
    backup: &Backup,
    request: &Request,
    opened: &Opened,
    outcome: &mut Outcome,
) -> Result<(), LibError> {
    let server_id = field(&backup.server, "appInstanceId");
    if !backup.service.starts_with(SERVICE_PACKAGE) {
        return Err(fail(format!(
            "AppInstance {server_id}'s kind names a backup service outside {SERVICE_PACKAGE}"
        )));
    }
    let action_id = actions::record(
        facade,
        &server_id,
        "backup",
        actions::RUNNING,
        opened.lease_id.as_deref(),
        opened.machine,
        "",
    )?;
    set_fields(
        facade,
        &action_id,
        &[("gitDeploymentId", Value::string(&request.git_deployment_id))],
    )?;
    let mut parameters: BTreeMap<String, Value> = [("appInstanceId".to_string(), Value::string(&server_id))].into();
    if let Some(database) = &backup.database {
        parameters.insert("databaseName".to_string(), Value::string(database));
    }
    match under(opened, || crate::lifecycle::call(&backup.service, &parameters)) {
        Ok(out) => {
            let file = out.get("backupFile").map(to_plain_string).unwrap_or_default();
            let line = format!("backed up AppInstance {server_id} to {file} before the change");
            actions::finish(facade, &action_id, actions::DONE, &line)?;
            outcome.report.push(line);
            Ok(())
        }
        Err(err) => {
            let line = format!(
                "the backup of AppInstance {server_id} failed, so nothing was changed: {}",
                err.message()
            );
            actions::finish(facade, &action_id, actions::FAILED, &line)?;
            Err(fail(line))
        }
    }
}

/// One instance's change (§4.6, step 6), recorded as its action.
fn change(
    facade: &EntityFacade,
    link: &dyn Link,
    plan: &DeployPlan,
    target: &Target,
    request: &Request,
    opened: &Opened,
    outcome: &mut Outcome,
) -> Result<(), LibError> {
    let id = target.id();
    let action_id = actions::record(
        facade,
        &id,
        request.verb(),
        actions::RUNNING,
        opened.lease_id.as_deref(),
        opened.machine,
        "",
    )?;
    set_fields(
        facade,
        &action_id,
        &[
            ("gitDeploymentId", Value::string(&request.git_deployment_id)),
            (
                "componentCommitsBefore",
                Value::string(json!(target.components_before).to_string()),
            ),
            ("schemaHashBefore", Value::string(&target.schema_before)),
            (
                "rollbackOfActionId",
                Value::string(plan.rollback_of.clone().unwrap_or_default()),
            ),
            (
                "instanceImageIdBefore",
                Value::string(field(&target.instance, "instanceImageId")),
            ),
        ],
    )?;
    let mut report = Vec::new();
    let changed = (|| -> Result<LinkStatus, LibError> {
        if target.drain {
            link.drain(&id, true)?;
            wait_drained(link, &id)?;
            report.push(format!("drained AppInstance {id}"));
        }
        let changed = apply(facade, link, target, request, opened, &mut report);
        if target.drain {
            // undrained after a failure too: the instance runs what it ran before, or what was put back
            match link.drain(&id, false) {
                Ok(()) => report.push(format!("undrained AppInstance {id}")),
                Err(err) => {
                    let line = format!("AppInstance {id} was not undrained: {}", err.message());
                    tracing::error!("Instances: {line}");
                    report.push(line);
                    if changed.is_ok() {
                        return Err(fail(line_of(&report)));
                    }
                }
            }
        }
        changed
    })();
    match changed {
        Ok(status) => {
            let mut row = plan::instance(facade, &id)?;
            row.set(
                "componentCommits",
                Value::string(json!(target.components_after).to_string()),
            );
            if !status.schema_hash.is_empty() {
                row.set("schemaHash", Value::string(&status.schema_hash));
            }
            // the image row it now runs, which a later rollback's pull takes its registry login from
            if let Some(image) = target.image_row.as_ref().filter(|_| target.image_after.is_some()) {
                row.set("instanceImageId", Value::string(field(image, "instanceImageId")));
            }
            row.update(facade)?;
            annotate(target, request, &status, &mut report);
            actions::finish(facade, &action_id, actions::DONE, &report.join("\n"))?;
            set_fields(
                facade,
                &action_id,
                &[
                    (
                        "componentCommitsAfter",
                        Value::string(json!(target.components_after).to_string()),
                    ),
                    ("schemaHashAfter", Value::string(&status.schema_hash)),
                ],
            )?;
            outcome.action_ids.push(action_id);
            outcome.instance_ids.push(id);
            outcome.report.extend(report);
            Ok(())
        }
        Err(err) => {
            report.push(format!("failed: {}", err.message()));
            actions::finish(facade, &action_id, actions::FAILED, &report.join("\n"))?;
            outcome.action_ids.push(action_id.clone());
            outcome.report.extend(report);
            Err(fail(format!(
                "{} (action {action_id}); the instances after it were not changed",
                err.message()
            )))
        }
    }
}

fn line_of(report: &[String]) -> String {
    report.last().cloned().unwrap_or_default()
}

/// The change itself: a new container from the new image (the old one, set aside, put back when the new one is not made,
/// does not start or does not come up), then each component's update over the link; then the wait until it is up.
fn apply(
    facade: &EntityFacade,
    link: &dyn Link,
    target: &Target,
    request: &Request,
    opened: &Opened,
    report: &mut Vec<String>,
) -> Result<LinkStatus, LibError> {
    let id = target.id();
    if let Some(image) = &target.image_after {
        let swap = under(opened, || replace(facade, target, image, report))?;
        match under(opened, || wait_up(facade, link, target)) {
            Ok(_) => under(opened, || swap.drop_old(report)),
            Err(err) => {
                report.push(format!(
                    "AppInstance {id} did not come up on {image}: {}",
                    err.message()
                ));
                under(opened, || swap.put_back(facade, report))?;
                under(opened, || wait_up(facade, link, target))?;
                return Err(err);
            }
        }
    }
    for component in &target.component_deploys {
        let answer = link.deploy_component(&id, &attestation(component, request))?;
        report.push(format!(
            "component {} at {} on AppInstance {id}: {}",
            component.name,
            short(&component.commit_id),
            if answer.is_empty() {
                "deployed".to_string()
            } else {
                answer
            }
        ));
    }
    under(opened, || wait_up(facade, link, target))
}

/// Waits until a drained instance says so (its health `draining`), then `manage@drain-wait-seconds` (30) for the
/// requests it was serving to end, before it is changed.
fn wait_drained(link: &dyn Link, id: &str) -> Result<(), LibError> {
    let seconds: u64 = moqui_core::context::conf_env::setting("manage@deploy-wait-seconds")
        .parse()
        .unwrap_or(600);
    let until = Instant::now() + Duration::from_secs(seconds);
    loop {
        let status = link.status(id)?;
        if status.health == "draining" {
            break;
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(fail(format!(
                "AppInstance {id} did not report itself drained in {seconds}s ({})",
                status.health
            )));
        }
        std::thread::sleep(POLL.min(left));
    }
    let grace: u64 = moqui_core::context::conf_env::setting("manage@drain-wait-seconds")
        .parse()
        .unwrap_or(30);
    std::thread::sleep(Duration::from_secs(grace));
    Ok(())
}

/// What the link's deploy carries: the commit as moqui-git's attestation names it, unsigned.
fn attestation(component: &Component, request: &Request) -> String {
    json!({
        "kind": ATTESTATION_KIND,
        "v": ATTESTATION_VERSION,
        "audience": "",
        "repositoryUrl": component.repository_url,
        "refName": component.ref_name,
        "commitId": component.commit_id,
        "component": component.name,
        "pipeline": "",
        "pipelineRunId": "",
        "runNum": 0,
        "runStatus": "success",
        "environment": request.environment_id,
        "deploymentId": if request.git_deployment_id.is_empty() { request.what() } else { request.git_deployment_id.clone() },
        "issuedAt": moqui_core::user::now_millis(),
        "source": request.source,
        "dataTypes": component.data_types,
        // only a rollback may go to a commit below the branch's head (moqui-devkit's deploy_managed)
        "rollback": matches!(request.change, Change::Rollback { .. }),
    })
    .to_string()
}

/// A replaced container: the new one, and the old one set aside under another name until the new one is up.
struct Swap {
    host: moqui_lib::docker::DockerHost,
    instance_id: String,
    name: String,
    old: Option<String>,
    made: String,
}

impl Swap {
    /// The new one is up: the old one goes (its volumes stay, they are the new one's).
    fn drop_old(&self, report: &mut Vec<String>) {
        if let Some(old) = &self.old {
            if let Err(err) = self.host.remove_container(old, false) {
                let line = format!(
                    "the old container of AppInstance {} was not removed: {err}",
                    self.instance_id
                );
                tracing::error!("Instances: {line}");
                report.push(line);
            }
        }
    }

    /// The new one did not come up: it goes, and the old one takes its name back and starts.
    fn put_back(&self, facade: &EntityFacade, report: &mut Vec<String>) -> Result<(), LibError> {
        if let Err(err) = self.host.remove_container(&self.made, false) {
            tracing::error!(
                "Instances: the new container of AppInstance {} was not removed: {err}",
                self.instance_id
            );
        }
        let Some(old) = &self.old else {
            return Ok(());
        };
        restore_old(&self.host, facade, &self.instance_id, old, &self.name)?;
        report.push(format!("AppInstance {}: its old container is back", self.instance_id));
        Ok(())
    }
}

/// The old container takes its name back and starts, and the instance's row names it again.
fn restore_old(
    host: &moqui_lib::docker::DockerHost,
    facade: &EntityFacade,
    instance_id: &str,
    old: &str,
    name: &str,
) -> Result<(), LibError> {
    host.rename(old, name)?;
    host.start(old)?;
    let mut row = plan::instance(facade, instance_id)?;
    row.set("instanceUuid", Value::string(old));
    row.update(facade)?;
    let info = host.inspect(old)?;
    crate::lifecycle::observe(facade, instance_id, info.as_ref())
}

/// Pulls `image` for a replacement: a new image always (with its registry login when it needs one), a rollback's
/// recorded digest only when the host no longer has it (with the login of the image it was, whose registry it is).
fn pull_for(host: &moqui_lib::docker::DockerHost, target: &Target, image: &str) -> Result<(), LibError> {
    let login = target
        .image_row
        .as_ref()
        .filter(|row| field(row, "registryLogin") == "Y");
    if target.rollback {
        if host.image_volumes(image)?.is_some() {
            return Ok(());
        }
        if image.starts_with("sha256:") {
            return Err(fail(format!(
                "The image {image} is no longer on the host and names no registry to pull it from"
            )));
        }
    } else if login.is_none() && host.image_volumes(image)?.is_some() {
        return Ok(());
    }
    match login {
        Some(row) => crate::provision::pull_with_login_ref(host, row, image),
        None => host.pull(image),
    }
}

/// Replaces the instance's container with one from `image` (§4.6, step 6): the image pulled, the old one stopped and
/// set aside under another name (its volumes kept), the new one made under the instance's name with the same volumes,
/// settings and secret files, and started. When the new one is not made or does not start, it goes and the old one
/// takes its name back and starts.
fn replace(facade: &EntityFacade, target: &Target, image: &str, report: &mut Vec<String>) -> Result<Swap, LibError> {
    let id = target.id();
    let row = plan::instance(facade, &id)?;
    let host = connect_host(&crate::lifecycle::host_of(facade, &row)?)?;
    pull_for(&host, target, image)?;
    let spec = crate::provision::replacement_spec(facade, &row, &target.kind, image)?;
    let name = spec.name.clone();
    let old = Some(field(&row, "instanceUuid")).filter(|old| !old.is_empty());
    if let Some(old) = &old {
        if let Err(err) = host.stop(old, STOP_GRACE) {
            tracing::warn!("Instances: the container of AppInstance {id} did not stop cleanly: {err}");
        }
        let mut tag = [0_u8; 4];
        moqui_vault::rng::Rng::fill(&mut moqui_vault::rng::OsRng, &mut tag);
        let aside = format!("{name}-replaced-{}", moqui_vault::text::hex(&tag));
        if let Err(err) = host.rename(old, &aside) {
            host.start(old)?;
            return Err(err);
        }
    }
    let mut made_id = None;
    let made = (|| -> Result<String, LibError> {
        let made = host.create_spec(&spec)?;
        made_id = Some(made.clone());
        host.start(&made)?;
        Ok(made)
    })();
    let made = match made {
        Ok(made) => made,
        Err(err) => {
            if let Some(made) = &made_id {
                if let Err(removed) = host.remove_container(made, false) {
                    tracing::error!("Instances: the new container of AppInstance {id} was not removed: {removed}");
                }
            }
            if let Some(old) = &old {
                restore_old(&host, facade, &id, old, &name)?;
                report.push(format!(
                    "AppInstance {id}: the new container failed, its old one is back"
                ));
            }
            return Err(err);
        }
    };
    let mut row = plan::instance(facade, &id)?;
    row.set("instanceUuid", Value::string(&made));
    row.update(facade)?;
    let info = host.inspect(&made)?;
    crate::lifecycle::observe(facade, &id, info.as_ref())?;
    report.push(format!(
        "AppInstance {id}: a new container ({}) from {image}, with its volumes and {} secret files",
        made.chars().take(12).collect::<String>(),
        spec.secrets.len()
    ));
    Ok(Swap {
        host,
        instance_id: id,
        name,
        old,
        made,
    })
}

/// Waits until the instance is up, at most `manage@deploy-wait-seconds`: an mr-moqui instance reports over its link that
/// it is healthy and unsealed; another's container runs and is healthy (or has no health check). Its status.
fn wait_up(facade: &EntityFacade, link: &dyn Link, target: &Target) -> Result<LinkStatus, LibError> {
    let id = target.id();
    let seconds: u64 = moqui_core::context::conf_env::setting("manage@deploy-wait-seconds")
        .parse()
        .unwrap_or(600);
    let until = Instant::now() + Duration::from_secs(seconds);
    let mut last: String;
    loop {
        if target.kind_id() == "mr-moqui" {
            match link.status(&id) {
                Ok(status) if status.ready() => return Ok(status),
                Ok(status) => last = format!("{}, {}", status.seal_state, status.health),
                Err(err) => last = err.message().to_string(),
            }
        } else {
            match crate::lifecycle::check_container(facade, &id) {
                Ok(Some(info)) if info.running && info.health.as_deref().is_none_or(|health| health == "healthy") => {
                    return Ok(LinkStatus::default())
                }
                Ok(Some(info)) => last = format!("{}, {}", info.status, info.health.unwrap_or_default()),
                Ok(None) => last = "its container is gone".to_string(),
                Err(err) => last = err.message().to_string(),
            }
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(fail(format!(
                "AppInstance {id} did not report healthy and unsealed in {seconds}s ({last})"
            )));
        }
        std::thread::sleep(POLL.min(left));
    }
}

/// The deploy's own annotation (§4.6, step 7): the service, the instance, the version and the revision. One that is not
/// posted is logged and reported.
fn annotate(target: &Target, request: &Request, status: &LinkStatus, report: &mut Vec<String>) {
    let id = target.id();
    let image = target
        .image_after
        .clone()
        .unwrap_or_else(|| target.image_before.clone());
    let revision = target
        .component_deploys
        .last()
        .map(|component| component.commit_id.clone())
        .or_else(|| {
            target
                .image_row
                .as_ref()
                .map(|image| field(image, "sourceRevision"))
                .filter(|revision| !revision.is_empty())
        })
        .unwrap_or_else(|| request.commit_id.clone());
    let version = match target.component_deploys.last() {
        Some(component) => format!("{}@{}", component.name, short(&component.commit_id)),
        None => image.clone(),
    };
    let service = Some(field(&target.instance, "hostName"))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| target.name());
    let mut parameters: BTreeMap<String, Value> = [
        ("source", "deploy".to_string()),
        (
            "text",
            format!(
                "{} {} to {}",
                if request.verb() == "rollback" {
                    "Rolled back"
                } else {
                    "Deployed"
                },
                request.what(),
                target.name()
            ),
        ),
        ("serviceName", service),
        ("instanceId", id.clone()),
        ("version", version),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), Value::string(value)))
    .collect();
    if !revision.is_empty() {
        parameters.insert("revision".to_string(), Value::string(revision));
    }
    if !status.schema_hash.is_empty() {
        parameters.insert(
            "tags".to_string(),
            Value::List(vec![Value::string(format!("schema:{}", short(&status.schema_hash)))]),
        );
    }
    if let Err(err) = crate::lifecycle::call(ANNOTATION, &parameters) {
        let line = format!("the annotation of AppInstance {id} was not posted: {}", err.message());
        tracing::warn!("Instances: {line}");
        report.push(line);
    }
}

// ----- from a screen, and from a GitDeployment -----

/// `request#InstanceDeploy`: a deploy by hand. Gated first; it runs now when it opens no vault item or the environment
/// has machine access; else it asks for its lease, records a waiting action on its first instance (the page goes to the
/// lease dialog), and runs on this member once an approver opens the lease.
pub fn request(
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    registry: &Arc<LeaseRegistry>,
    link: Arc<dyn Link>,
    request: Request,
    stored: &str,
) -> Result<Requested, LibError> {
    let plan = plan(facade, &*link, &request)?;
    let environment = one(facade, ENVIRONMENT, "environmentId", &plan.environment_id)?
        .ok_or_else(|| fail(format!("No InstanceEnvironment {}", plan.environment_id)))?;
    actions::check_caller(&environment)?;
    gate(facade, &environment, &request)?;
    let group = field(&environment, "approverUserGroupId");
    let second_person = !group.is_empty() && field(&environment, "singlePersonDeploy") != "Y";
    let leased = !plan.bindings.is_empty() && !plan.machine_access;
    if !leased && second_person {
        // no lease to open: another member of the group approves it (approve#InstanceDeploy), which runs it
        let first = plan.targets.first().map(Target::id).unwrap_or_default();
        let action_id = actions::record(
            facade,
            &first,
            request.verb(),
            actions::WAITING,
            None,
            false,
            &format!("waiting for another member of group {group} to approve it"),
        )?;
        set_fields(facade, &action_id, &[("deployRequest", Value::string(stored))])?;
        return Ok(Requested {
            action_id,
            status: actions::WAITING.to_string(),
            lease_id: None,
        });
    }
    if !leased {
        let outcome = run(facade, registry, &*link, &request, None)?;
        return Ok(Requested {
            action_id: outcome.action_ids.last().cloned().unwrap_or_default(),
            status: actions::DONE.to_string(),
            lease_id: None,
        });
    }
    let lease = registry.ask(
        facade,
        &Ask {
            vault_id: &plan.vault_id,
            bindings: &plan.bindings,
            operation: &plan.operation,
            limit: plan.lease_limit,
            environment_id: None,
        },
    )?;
    let lease_id = lease.lease_id.clone();
    let first = plan.targets.first().map(Target::id).unwrap_or_default();
    let action_id = actions::record(
        facade,
        &first,
        request.verb(),
        actions::WAITING,
        Some(&lease_id),
        false,
        &format!("waiting for an approver to open lease {lease_id}"),
    )?;
    let limit = Duration::from_millis(u64::try_from(lease.expires_at - moqui_core::user::now_millis()).unwrap_or(0));
    let (factory, registry) = (Arc::clone(factory), Arc::clone(registry));
    let waiting = action_id.clone();
    let thread_lease = lease_id.clone();
    let user = moqui_core::user::current_user_id().ok().flatten();
    let name = format!("instance-deploy-{first}");
    let start = move || -> std::io::Result<()> {
        std::thread::Builder::new().name(name).spawn(move || {
            let context = factory.new_context();
            let _entered = context.enter();
            let Some(facade) = factory.entity() else {
                return;
            };
            let _authz = crate::common::authz_off();
            if let Some(user) = &user {
                actions::sign_in_as(&facade, user, &format!("the waiting deploy {waiting}"));
            }
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if let Err(err) = registry.wait_open(&facade, &thread_lease, limit) {
                    registry.forget(&thread_lease);
                    return Err(err);
                }
                // a deploy's lease is approved by another than who asked for it
                let approver = one(&facade, "moqui.security.vault.VaultLease", "leaseId", &thread_lease)?
                    .map(|row| field(&row, "approvedByUserId"))
                    .unwrap_or_default();
                if user.as_deref().is_some_and(|user| user == approver) {
                    if let Err(err) = registry.end(&facade, &thread_lease) {
                        registry.forget(&thread_lease);
                        tracing::error!("Instances: the lease {thread_lease} was not marked ended: {err}");
                    }
                    return Err(fail(format!(
                        "the lease {thread_lease} was approved by {approver}, who asked for the deploy: another \
                         approves it (reasonCode selfApproval)"
                    )));
                }
                run(&facade, &registry, &*link, &request, Some(&thread_lease))
            }));
            let (status, report) = match ran {
                Ok(Ok(outcome)) => (
                    actions::DONE,
                    format!("ran as actions {}", outcome.action_ids.join(", ")),
                ),
                Ok(Err(err)) => (actions::FAILED, err.message().to_string()),
                Err(_) => (actions::FAILED, "the deploy failed".to_string()),
            };
            if let Err(err) = actions::finish(&facade, &waiting, status, &report) {
                tracing::error!("Instances: the waiting deploy {waiting} was not updated: {err}");
            }
        })?;
        Ok(())
    };
    // it waits on the action and lease rows this request makes: it starts once they are committed
    actions::start_after_commit(facade, &action_id, start)?;
    Ok(Requested {
        action_id,
        status: actions::WAITING.to_string(),
        lease_id: Some(lease_id),
    })
}

/// The deploy a `GitDeployment` asks for: a component's commit to the mr-moqui instances of the managed environment its
/// CI environment names. The component's name and URL are the deploy step's (moqui-git reads `component.xml` at the
/// commit); none when only its lease is planned.
pub fn git_request(
    facade: &EntityFacade,
    deployment_id: &str,
    component: Component,
    source: &str,
) -> Result<Request, LibError> {
    let deployment = one(facade, DEPLOYMENT, "deploymentId", deployment_id)?
        .ok_or_else(|| fail(format!("No GitDeployment {deployment_id}")))?;
    let ci_environment = one(
        facade,
        CI_ENVIRONMENT,
        "environmentId",
        &field(&deployment, "environmentId"),
    )?
    .ok_or_else(|| fail(format!("GitDeployment {deployment_id}'s environment is gone")))?;
    let environment_id = field(&ci_environment, "instanceEnvironmentId");
    if environment_id.is_empty() {
        return Err(fail(format!(
            "GitDeployment {deployment_id}'s environment names no managed instances' environment"
        )));
    }
    let commit = field(&deployment, "commitId");
    // the URL the CI environment names, when it names one: the instance fetches from there
    let clone_url = field(&ci_environment, "cloneUrl");
    let component = Component {
        ref_name: field(&deployment, "refName"),
        commit_id: commit.clone(),
        repository_url: if clone_url.is_empty() {
            component.repository_url
        } else {
            clone_url
        },
        ..component
    };
    let mut request = Request::new(&environment_id, Change::Component(component));
    request.commit_id = commit;
    request.repository_id = field(&deployment, "repositoryId");
    request.git_deployment_id = deployment_id.to_string();
    request.fallback_pipeline = field(&ci_environment, "requiredPipeline");
    request.source = source.to_string();
    Ok(request)
}

/// What [`prepare_git`] asked for.
#[derive(Debug, Clone, Default)]
pub struct Prepared {
    pub lease_id: Option<String>,
    /// A person approves the deployment: the environment has no machine access.
    pub approval_required: bool,
    pub lease_minutes: u64,
}

/// `prepare#GitDeployment`: once a deployment's pipeline passed, the gate, and the lease of what its deploy opens, held
/// on this member, which then runs its deploy step. None when the environment has machine access, or it opens nothing;
/// its approval is required unless the environment has machine access.
pub fn prepare_git(facade: &EntityFacade, registry: &LeaseRegistry, deployment_id: &str) -> Result<Prepared, LibError> {
    let request = git_request(facade, deployment_id, Component::default(), "")?;
    let plan = plan(facade, &ServiceLink, &request)?;
    let environment = one(facade, ENVIRONMENT, "environmentId", &plan.environment_id)?
        .ok_or_else(|| fail(format!("No InstanceEnvironment {}", plan.environment_id)))?;
    gate(facade, &environment, &request)?;
    let lease_minutes = plan.lease_limit.map_or(240, |limit| limit.as_secs() / 60);
    if plan.machine_access {
        return Ok(Prepared {
            lease_minutes,
            ..Prepared::default()
        });
    }
    if plan.bindings.is_empty() {
        return Ok(Prepared {
            lease_id: None,
            approval_required: true,
            lease_minutes,
        });
    }
    let lease = registry.ask(
        facade,
        &Ask {
            vault_id: &plan.vault_id,
            bindings: &plan.bindings,
            operation: &plan.operation,
            limit: plan.lease_limit,
            environment_id: None,
        },
    )?;
    Ok(Prepared {
        lease_id: Some(lease.lease_id),
        approval_required: true,
        lease_minutes,
    })
}

/// The JSON a deployment keeps of a deploy to managed instances (`deployResult`).
pub fn result_json(request: &Request, outcome: &Outcome) -> String {
    let (component, commit) = match &request.change {
        Change::Component(component) => (component.name.clone(), component.commit_id.clone()),
        _ => (String::new(), request.commit_id.clone()),
    };
    json!({
        "status": "deployed",
        "component": component,
        "commitId": commit,
        "instances": outcome.instance_ids,
        "actions": outcome.action_ids,
        "problems": [],
    })
    .to_string()
}
