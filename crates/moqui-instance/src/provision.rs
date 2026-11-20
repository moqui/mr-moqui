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

//! A provision (plans/Vault-Instances.md §4.3, step 4), under a lease or the environment's machine access
//! ([`crate::actions::open_authority`]), each instance of the plan in order, dependencies first:
//!
//! 1. The credentials the kind makes (a database's admin password, an mr-moqui instance's database password and
//!    ingest key) are made here and deposited into the environment's vault: wrapped to its deposit key, so the server
//!    writes them and cannot read them back, with no lease ([`moqui_vault_server::deposit::deposit_item`]).
//! 2. The database and user on its database dependency, with the admin login: made just now (held in memory for this
//!    provision) or leased (`ec.vault.leased`).
//! 3. Its network, volumes and container, each secret a file under `/run/secrets` (an archive put before the start on
//!    Docker, Podman secrets on Podman), never in the container's environment, labels or command: the driver's check
//!    refuses that before anything is sent. A new mr-moqui instance's one-time pairing token is a file too; its hash
//!    and its 15 minutes are on the instance's row, for the manager link (M2).
//! 4. The container started and checked.
//!
//! The action row records what was made; a failure leaves what was made (the container's id is on the row as soon as
//! it exists, so Remove removes it) and says where it stopped.

use std::collections::BTreeMap;
use std::time::Duration;

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_lib::docker::{encode, DockerHost, PULL_TIMEOUT};
use moqui_lib::error::Error as LibError;
use moqui_lib::http::Method;
use moqui_lib::object::Value;
use moqui_vault_server::deposit::{deposit_item, Deposit};
use moqui_vault_server::lease::LeaseRegistry;
use serde_json::json;
use sha2::Digest;
use zeroize::Zeroizing;

use crate::actions::{self, Opened};
use crate::common::{
    connect_host, fail, field, host_is_local, leased, new_credential, new_item_id, now, rows, INSTANCE_ENV,
    INSTANCE_HOST_CONFIG, INSTANCE_SECRET, INSTANCE_VOLUME,
};
use crate::kinds::{self, AdminLogin, ContainerInput};
use crate::plan::{self, AdminSource, Plan, Step};

/// How long a provision waits for a database it made to take connections.
pub const READY_WAIT: Duration = Duration::from_secs(120);

/// What an action did: its row, and its report.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub action_id: String,
    pub report: Vec<String>,
}

/// Provisions `instance_id` and the dependencies it needs that are not made yet, under the lease `lease_id` or the
/// environment's machine access when its plan opens vault items (see the module doc).
pub fn provision(
    facade: &EntityFacade,
    registry: &LeaseRegistry,
    instance_id: &str,
    lease_id: Option<&str>,
) -> Result<Outcome, LibError> {
    let plan = plan::build(facade, instance_id)?;
    if plan.steps.is_empty() {
        return Err(fail(format!(
            "AppInstance {instance_id} has its container: it is provisioned"
        )));
    }
    let operation = plan::operation("provision", instance_id);
    let opened = match actions::open_authority(
        facade,
        registry,
        &plan.environment_id,
        &plan.vault_id,
        plan.machine_access,
        &plan.lease_bindings,
        &operation,
        lease_id,
    ) {
        Ok(opened) => opened,
        Err(err) => {
            actions::record(
                facade,
                instance_id,
                "provision",
                actions::REFUSED,
                lease_id,
                false,
                err.message(),
            )?;
            return Err(err);
        }
    };
    let action_id = actions::record(
        facade,
        instance_id,
        "provision",
        actions::RUNNING,
        opened.lease_id.as_deref(),
        opened.machine,
        "",
    )?;
    let mut report = Vec::new();
    let ran = match &opened.lease_id {
        Some(lease_id) => moqui_vault_server::lease::run_under(lease_id, || execute(facade, &plan, &mut report)),
        None => execute(facade, &plan, &mut report),
    };
    actions::end_lease(facade, registry, &opened);
    finish(facade, &action_id, &opened, ran, report)
}

/// Writes an action's end and returns its outcome, or its error with what was done before it.
pub(crate) fn finish(
    facade: &EntityFacade,
    action_id: &str,
    _opened: &Opened,
    ran: Result<(), LibError>,
    mut report: Vec<String>,
) -> Result<Outcome, LibError> {
    match ran {
        Ok(()) => {
            actions::finish(facade, action_id, actions::DONE, &report.join("\n"))?;
            Ok(Outcome {
                action_id: action_id.to_string(),
                report,
            })
        }
        Err(err) => {
            report.push(format!("failed: {}", err.message()));
            actions::finish(facade, action_id, actions::FAILED, &report.join("\n"))?;
            Err(fail(format!("{} (action {action_id})", err.message())))
        }
    }
}

/// The admin login of a database server this provision made: its user and password, in memory only.
struct MadeAdmin {
    user: String,
    password: Zeroizing<String>,
}

fn execute(facade: &EntityFacade, plan: &Plan, report: &mut Vec<String>) -> Result<(), LibError> {
    let mut made_admins: BTreeMap<String, MadeAdmin> = BTreeMap::new();
    for step in &plan.steps {
        let values = deposit_credentials(facade, plan, step, report)?;
        if let Some(admin) = values.get("admin") {
            let user = if field(&step.kind, "confName") == "postgres" {
                "postgres"
            } else {
                "root"
            };
            made_admins.insert(
                step.id(),
                MadeAdmin {
                    user: user.to_string(),
                    password: admin.clone(),
                },
            );
        }
        if let Some(database) = &step.database {
            make_database(database, &values, &made_admins, report)?;
        }
        make_container(facade, plan, step, &values, report)?;
    }
    Ok(())
}

/// Makes each credential the instance's kind generates and deposits it into the environment's vault; the values, by
/// secret name, for this provision only.
fn deposit_credentials(
    facade: &EntityFacade,
    plan: &Plan,
    step: &Step,
    report: &mut Vec<String>,
) -> Result<BTreeMap<String, Zeroizing<String>>, LibError> {
    let id = step.id();
    let kind = field(&step.kind, "instanceKindId");
    let name = field(&step.instance, "instanceName");
    let mut values = BTreeMap::new();
    for secret in step.secrets.iter().filter(|secret| field(secret, "generated") == "Y") {
        let secret_name = field(secret, "secretName");
        let binding = field(secret, "bindingKey");
        let item_type = field(secret, "vaultItemTypeId");
        let field_name = Some(field(secret, "fieldName"))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "password".to_string());
        let value = kinds::credential_for(&kind);
        let mut fields = serde_json::Map::new();
        match (item_type.as_str(), secret_name.as_str()) {
            ("database-credential", "admin") => {
                let user = if field(&step.kind, "confName") == "postgres" {
                    "postgres"
                } else {
                    "root"
                };
                fields.insert("host".to_string(), json!(name));
                fields.insert("port".to_string(), json!(field(&step.kind, "containerPort")));
                fields.insert("username".to_string(), json!(user));
            }
            ("database-credential", _) => {
                if let Some(database) = &step.database {
                    fields.insert("host".to_string(), json!(database.host));
                    fields.insert("port".to_string(), json!(database.port));
                    fields.insert("database".to_string(), json!(database.database));
                    fields.insert("username".to_string(), json!(database.user));
                }
            }
            ("login", _) => {
                fields.insert("username".to_string(), json!("admin"));
            }
            ("api-key", _) => {
                fields.insert("keyId".to_string(), json!(format!("{name}-{secret_name}")));
            }
            _ => {}
        }
        fields.insert(field_name, json!(value.as_str()));
        let body = Zeroizing::new(
            json!({ "v": 1, "type": item_type, "name": format!("{name} {secret_name}"), "fields": fields }).to_string(),
        );
        deposit_item(
            facade,
            &Deposit {
                vault_id: &plan.vault_id,
                item_id: &new_item_id(),
                item_type: &item_type,
                binding: &binding,
                name: &format!("{name} {secret_name}"),
                body: body.as_bytes(),
            },
        )
        .map_err(|err| fail(format!("AppInstance {id}: depositing {binding}: {}", err.message())))?;
        let mut row = secret.clone();
        row.set("depositedDate", now());
        row.update(facade)?;
        report.push(format!("deposited {binding} into vault {}", plan.vault_id));
        values.insert(secret_name, value);
    }
    Ok(values)
}

/// Makes the step's database and user on its database dependency with the admin login.
fn make_database(
    database: &plan::DatabaseUse,
    values: &BTreeMap<String, Zeroizing<String>>,
    made_admins: &BTreeMap<String, MadeAdmin>,
    report: &mut Vec<String>,
) -> Result<(), LibError> {
    let password = values.get(&database.password_secret).ok_or_else(|| {
        fail(format!(
            "The database user {}'s password is not made by this provision: its kind names no {} secret",
            database.user, database.password_secret
        ))
    })?;
    let (user, admin_password) = match &database.admin {
        AdminSource::Made(other) => {
            let made = made_admins
                .get(other)
                .ok_or_else(|| fail(format!("AppInstance {other} made no admin login")))?;
            (made.user.clone(), made.password.clone())
        }
        AdminSource::Leased(binding) => {
            let user = leased(binding, "username")?;
            (user.to_string(), leased(binding, "password")?)
        }
    };
    let admin = AdminLogin {
        conf_name: database.conf_name.clone(),
        host: database.host.clone(),
        port: database.port.clone(),
        user,
        password: admin_password,
        ssl_mode: database.ssl_mode.clone(),
    };
    if database.wait {
        kinds::wait_ready(&admin, READY_WAIT)?;
    }
    let made = kinds::create_database(&admin, &database.database, &database.user, password)?;
    report.push(format!(
        "database {} {} and user {} {} on {}",
        database.database,
        if made.database_created { "made" } else { "was there" },
        database.user,
        if made.user_created { "made" } else { "was there" },
        database.on
    ));
    Ok(())
}

/// Pulls an image with the registry login of the lease (`X-Registry-Auth`, which the engine passes to the registry).
fn pull_with_login(host: &DockerHost, image: &EntityValue) -> Result<(), LibError> {
    use base64::Engine as _;
    let image_id = field(image, "instanceImageId");
    let binding = format!("InstanceImage:{image_id}:registry");
    let user = leased(&binding, "username")?;
    let password = leased(&binding, "password")?;
    let auth = Zeroizing::new(
        json!({ "username": user.as_str(), "password": password.as_str(), "serveraddress": field(image, "registryLocation") })
            .to_string(),
    );
    let header = Zeroizing::new(base64::engine::general_purpose::URL_SAFE.encode(auth.as_bytes()));
    let name = field(image, "imageName");
    let (repository, tag) = match name.rsplit_once(':') {
        Some((repository, tag)) if !tag.contains('/') => (repository.to_string(), tag.to_string()),
        _ => (name.clone(), "latest".to_string()),
    };
    let response = host
        .client()
        .request(
            Method::Post,
            host.url(&format!(
                "/images/create?fromImage={}&tag={}",
                encode(&repository),
                encode(&tag)
            )),
        )
        .header("X-Registry-Auth", header.as_str())
        .timeout(PULL_TIMEOUT)
        .allow_in_sim(true)
        .send(false)?;
    if response.status() >= 300 {
        return Err(DockerHost::problem(&format!("pulling image {name}"), &response));
    }
    if let Some(line) = response.body().lines().rev().find(|line| line.contains("\"error\"")) {
        return Err(fail(format!("pulling image {name}: {line}")));
    }
    Ok(())
}

/// Whether an engine's refusal says the network or volume is there already.
fn already_there(err: &LibError) -> bool {
    let message = err.message().to_ascii_lowercase();
    message.contains("409") || message.contains("already exists")
}

/// Makes the step's network, volumes and container (secrets as files), starts it and records what its host says.
fn make_container(
    facade: &EntityFacade,
    plan: &Plan,
    step: &Step,
    values: &BTreeMap<String, Zeroizing<String>>,
    report: &mut Vec<String>,
) -> Result<(), LibError> {
    let id = step.id();
    let host = connect_host(&step.host)?;
    let host_id = field(&step.host, "instanceHostId");
    let login = field(&step.image, "registryLogin") == "Y";
    if login {
        pull_with_login(&host, &step.image)?;
    }
    let labels = vec![
        ("moqui.instance".to_string(), id.clone()),
        ("moqui.environment".to_string(), plan.environment_id.clone()),
    ];
    let network = Some(field(&step.instance, "networkName")).filter(|network| !network.is_empty());
    if let Some(network) = &network {
        let label_map = json!({ "moqui.environment": plan.environment_id });
        match host.create_network(network, &label_map, false) {
            Ok(()) => report.push(format!("network {network} on host {host_id}")),
            Err(err) if already_there(&err) => {}
            Err(err) => return Err(err),
        }
    }
    let mut volumes = Vec::new();
    for volume in rows(facade, INSTANCE_VOLUME, "appInstanceId", &id)? {
        let name = field(&volume, "volumeName");
        match host.create_volume(&name, &labels) {
            Ok(()) => report.push(format!("volume {name} on host {host_id}")),
            Err(err) if already_there(&err) => {}
            Err(err) => return Err(err),
        }
        volumes.push((field(&volume, "mountPoint"), name));
    }
    let env = rows(facade, INSTANCE_ENV, "appInstanceId", &id)?
        .iter()
        .map(|row| (field(row, "envName"), field(row, "envValue")))
        .filter(|(name, _)| !name.is_empty())
        .collect();
    let host_config = rows(facade, INSTANCE_HOST_CONFIG, "appInstanceId", &id)?
        .iter()
        .map(|row| (field(row, "hostConfigName"), field(row, "hostConfigValue")))
        .collect();
    let mut secrets = Vec::new();
    for secret in rows(facade, INSTANCE_SECRET, "appInstanceId", &id)? {
        let file = field(&secret, "fileName");
        if file.is_empty() {
            continue;
        }
        let secret_name = field(&secret, "secretName");
        let value = match values.get(&secret_name) {
            Some(value) => value.clone(),
            None => leased(&field(&secret, "bindingKey"), &field(&secret, "fieldName"))?,
        };
        let setting = Some(field(&secret, "envName")).filter(|setting| !setting.is_empty());
        secrets.push((file, setting, Zeroizing::new(value.as_bytes().to_vec())));
    }
    let mut instance = step.instance.clone();
    if field(&step.kind, "instanceKindId") == "mr-moqui" {
        // the one-time pairing token (§4.2): a file, as the other secrets are; only its hash is kept
        let token = new_credential();
        let hash = moqui_vault::text::hex(&sha2::Sha256::digest(token.as_bytes()));
        instance.set("pairingTokenHash", Value::string(hash));
        let expires = moqui_core::user::now_millis() + kinds::PAIRING_TOKEN_MINUTES * 60 * 1000;
        instance.set("pairingTokenExpires", moqui_core::user::stamp_value(expires));
        secrets.push((
            kinds::PAIRING_TOKEN_FILE.to_string(),
            None,
            Zeroizing::new(token.as_bytes().to_vec()),
        ));
    }
    let port = |name: &str, row: &EntityValue| field(row, name).parse::<u16>().ok();
    let spec = kinds::container_spec(ContainerInput {
        kind: field(&step.kind, "instanceKindId"),
        name: field(&step.instance, "instanceName"),
        image: field(&step.image, "imageName"),
        pull: !login,
        env,
        volumes,
        host_config,
        labels,
        network,
        container_port: port("containerPort", &step.kind),
        host_port: port("hostPort", &step.instance),
        host_ip: host_is_local(&step.host).then(|| "127.0.0.1".to_string()),
        run_as: Some(field(&step.kind, "runAsUser")).filter(|run_as| !run_as.is_empty()),
        secrets,
    })?;
    let container = host.create_spec(&spec)?;
    instance.set("instanceUuid", Value::string(&container));
    instance.set("desiredStateEnumId", Value::string("IdsRunning"));
    instance.update(facade)?;
    report.push(format!(
        "container {} ({}) on host {host_id}, with {} secret files",
        spec.name,
        container.chars().take(12).collect::<String>(),
        spec.secrets.len()
    ));
    host.start(&container)?;
    let info = host.inspect(&container)?;
    crate::lifecycle::observe(facade, &id, info.as_ref())?;
    match info {
        Some(info) if info.running => report.push(format!("container {} started", spec.name)),
        Some(info) => {
            return Err(fail(format!(
                "container {} is {} after its start",
                spec.name, info.status
            )))
        }
        None => return Err(fail(format!("container {} is gone after its start", spec.name))),
    }
    Ok(())
}
