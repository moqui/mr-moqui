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
//! 2. The database and user on its database dependency, with the admin login (made just now and held in memory, or
//!    leased and checked against the server its own item names): from a one-shot client container on the server's
//!    network, or over verified TLS to a database host ([`crate::database`]). A user or database that is there already
//!    is refused.
//! 3. Its network, volumes and container, each secret a file under `/run/secrets` (an archive put before the start on
//!    Docker, Podman secrets on Podman), never in the container's environment, labels or command: the driver's check
//!    refuses that before anything is sent. A published port is bound to the host's loopback, never every address. A
//!    new mr-moqui instance's one-time pairing token is made after its image is pulled, as a file; its hash and its 15
//!    minutes are on the instance's row, for the manager link (M2).
//! 4. The container started and checked.
//!
//! **All or nothing.** A volume that is there already is refused before anything is made (its data would hold another
//! instance's credentials). When a step fails, what this provision made is undone in reverse: its containers and
//! volumes removed, a database and user it made on a server it did not make dropped, its deposits withdrawn (they are
//! still wrapped to the deposit key, so no member has taken them in), so the provision can run again. Each undo that
//! fails is logged and named in the report, and the rest go on.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_lib::docker::{encode, ContainerSpec, DockerHost, RestartPolicy, SecretFile, PULL_TIMEOUT};
use moqui_lib::error::Error as LibError;
use moqui_lib::http::Method;
use moqui_lib::object::Value;
use moqui_vault_server::deposit::{deposit_item, withdraw_item, Deposit};
use moqui_vault_server::lease::LeaseRegistry;
use serde_json::json;
use sha2::Digest;
use zeroize::Zeroizing;

use crate::actions::{self, Opened};
use crate::common::{
    connect_host, fail, field, leased, new_credential, new_item_id, now, rows, INSTANCE_ENV, INSTANCE_HOST_CONFIG,
    INSTANCE_SECRET, INSTANCE_VOLUME,
};
use crate::database::{self, AdminLogin, OneShot, Server, EXIT_EXISTS, ONE_SHOT_WAIT};
use crate::kinds::{self, ContainerInput};
use crate::plan::{self, AdminSource, DatabaseUse, Plan, Reach, Step};

/// The label of a one-shot client container (a fake engine's one-shot exits as soon as it starts).
pub const ONE_SHOT_LABEL: &str = "moqui.one-shot";
/// The address a published port listens on: the host's loopback, never every address.
const PUBLISH_ON: &str = "127.0.0.1";

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
    let environment = plan::environment_of(facade, &plan::instance(facade, instance_id)?)?;
    actions::check_caller(&environment)?;
    let opened = match actions::open_authority(
        facade,
        registry,
        &plan.environment_id,
        &plan.vault_id,
        plan.machine_access,
        &plan.lease_bindings,
        &plan.operation,
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

/// What this provision made, to undo in reverse when a later step fails.
enum Made {
    Deposit {
        vault_id: String,
        item_id: String,
        binding: String,
        secret: EntityValue,
    },
    Database {
        use_: Box<DatabaseUse>,
        admin: AdminLogin,
    },
    Volume {
        host: EntityValue,
        name: String,
    },
    Container {
        host: EntityValue,
        instance_id: String,
        container: String,
    },
}

fn execute(facade: &EntityFacade, plan: &Plan, report: &mut Vec<String>) -> Result<(), LibError> {
    preflight(facade, plan)?;
    let mut made: Vec<Made> = Vec::new();
    let ran = (|| -> Result<(), LibError> {
        let mut made_admins: BTreeMap<String, MadeAdmin> = BTreeMap::new();
        let mut ingest_keys = Vec::new();
        for step in &plan.steps {
            let values = deposit_credentials(facade, plan, step, report, &mut made)?;
            if let Some(key) = values.get(crate::monitor::INGEST_SECRET) {
                ingest_keys.push((step.id(), key.clone()));
            }
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
                make_database(step, database, &values, &made_admins, report, &mut made)?;
            }
            make_container(facade, plan, step, &values, report, &mut made)?;
        }
        // step 6, register (§4.3, §4.8): each ingest key and scrape target, once every instance is up
        crate::monitor::register(facade, plan, &ingest_keys, report);
        Ok(())
    })();
    if ran.is_err() {
        undo(facade, made, report);
    }
    ran
}

/// What is refused before anything is made: a volume that is there already (an earlier instance's data, with its own
/// credentials), and a credential an earlier provision deposited and did not undo.
fn preflight(facade: &EntityFacade, plan: &Plan) -> Result<(), LibError> {
    for step in &plan.steps {
        let id = step.id();
        if let Some(secret) = step
            .secrets
            .iter()
            .find(|secret| field(secret, "generated") == "Y" && !field(secret, "depositedDate").is_empty())
        {
            return Err(fail(format!(
                "AppInstance {id}: {} was deposited by an earlier provision that did not finish: withdraw the item in \
                 System › Vaults and clear its binding before provisioning again (reasonCode depositKept)",
                field(secret, "bindingKey")
            )));
        }
        let volumes = rows(facade, INSTANCE_VOLUME, "appInstanceId", &id)?;
        if volumes.is_empty() {
            continue;
        }
        let host = connect_host(&step.host)?;
        for volume in volumes {
            let name = field(&volume, "volumeName");
            if host.inspect_volume(&name)?.is_some() {
                return Err(fail(format!(
                    "AppInstance {id}: the volume {name} is there already, with an earlier instance's data and \
                     credentials: remove it, or give the instance another name (reasonCode volumeKept)"
                )));
            }
        }
    }
    Ok(())
}

/// Undoes what this provision made, last first; a failure is logged and reported, and the rest go on.
fn undo(facade: &EntityFacade, made: Vec<Made>, report: &mut Vec<String>) {
    for thing in made.into_iter().rev() {
        let (what, done) = match thing {
            Made::Container {
                host,
                instance_id,
                container,
            } => {
                let done = connect_host(&host)
                    .and_then(|host| host.remove_container(&container, true))
                    .and_then(|()| {
                        crate::link::drop_bundle(&instance_id);
                        let mut row = plan::instance(facade, &instance_id)?;
                        for name in [
                            "instanceUuid",
                            "pairingTokenHash",
                            "pairingTokenExpires",
                            "desiredStateEnumId",
                        ] {
                            row.set(name, Value::Null);
                        }
                        row.set("observedState", Value::string("missing"));
                        row.update(facade)?;
                        Ok(())
                    });
                (
                    format!("container {}", container.chars().take(12).collect::<String>()),
                    done,
                )
            }
            Made::Volume { host, name } => {
                let done = connect_host(&host).and_then(|host| host.remove_volume(&name));
                (format!("volume {name}"), done)
            }
            Made::Database { use_, admin } => {
                let done = drop_on(&use_, &admin);
                (
                    format!("database {} and user {} on {}", use_.database, use_.user, use_.on),
                    done,
                )
            }
            Made::Deposit {
                vault_id,
                item_id,
                binding,
                mut secret,
            } => {
                let done = withdraw_item(facade, &vault_id, &item_id, &binding).and_then(|()| {
                    secret.set("depositedDate", Value::Null);
                    secret.update(facade)?;
                    Ok(())
                });
                (format!("deposit {binding}"), done)
            }
        };
        match done {
            Ok(()) => report.push(format!("undone: {what}")),
            Err(err) => {
                tracing::error!("Instances: {what} was not undone: {err}");
                report.push(format!("not undone: {what}: {}", err.message()));
            }
        }
    }
}

/// Makes each credential the instance's kind generates and deposits it into the environment's vault; the values, by
/// secret name, for this provision only.
fn deposit_credentials(
    facade: &EntityFacade,
    plan: &Plan,
    step: &Step,
    report: &mut Vec<String>,
    made: &mut Vec<Made>,
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
        let value = if secret_name == crate::monitor::INGEST_SECRET {
            crate::monitor::new_ingest_key()
        } else {
            kinds::credential_for(&kind)
        };
        let mut fields = serde_json::Map::new();
        match (item_type.as_str(), secret_name.as_str()) {
            ("database-credential", "admin") => {
                let user = if field(&step.kind, "confName") == "postgres" {
                    "postgres"
                } else {
                    "root"
                };
                // the server as its clients on the network reach it, which a later lease of this login is checked against
                fields.insert("host".to_string(), json!(name));
                fields.insert("port".to_string(), json!(field(&step.kind, "containerPort")));
                fields.insert("username".to_string(), json!(user));
            }
            ("database-credential", _) => {
                if let Some(database) = &step.database {
                    // what the instance's container uses: the server's name on the network, or the host's address
                    fields.insert("host".to_string(), json!(database.container_host));
                    fields.insert("port".to_string(), json!(database.container_port));
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
        let item_id = new_item_id();
        deposit_item(
            facade,
            &Deposit {
                vault_id: &plan.vault_id,
                item_id: &item_id,
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
        made.push(Made::Deposit {
            vault_id: plan.vault_id.clone(),
            item_id,
            binding: binding.clone(),
            secret: row,
        });
        report.push(format!("deposited {binding} into vault {}", plan.vault_id));
        values.insert(secret_name, value);
    }
    Ok(values)
}

/// A leased database admin login, refused when the item's own server is not the one the row names (the row was
/// changed, or the item is another server's): the login goes only where its item says.
pub(crate) fn leased_admin(database: &DatabaseUse, binding: &str) -> Result<AdminLogin, LibError> {
    let host = leased(binding, "host").unwrap_or_default();
    let port = leased(binding, "port").unwrap_or_default();
    if host.as_str() != database.expect_host || port.as_str() != database.expect_port {
        return Err(fail(format!(
            "{binding} names the server {}:{}, not {}:{} where it would be used: the login goes only to its own \
             server (reasonCode destinationMismatch)",
            host.as_str(),
            port.as_str(),
            database.expect_host,
            database.expect_port
        )));
    }
    Ok(AdminLogin {
        user: leased(binding, "username")?.to_string(),
        password: leased(binding, "password")?,
    })
}

fn admin_of(database: &DatabaseUse, made_admins: &BTreeMap<String, MadeAdmin>) -> Result<AdminLogin, LibError> {
    match &database.admin {
        AdminSource::Made(other) => {
            let made = made_admins
                .get(other)
                .ok_or_else(|| fail(format!("AppInstance {other} made no admin login")))?;
            Ok(AdminLogin {
                user: made.user.clone(),
                password: made.password.clone(),
            })
        }
        AdminSource::Leased(binding) => leased_admin(database, binding),
    }
}

/// Makes the step's database and user on its database dependency with the admin login.
fn make_database(
    step: &Step,
    database: &DatabaseUse,
    values: &BTreeMap<String, Zeroizing<String>>,
    made_admins: &BTreeMap<String, MadeAdmin>,
    report: &mut Vec<String>,
    made: &mut Vec<Made>,
) -> Result<(), LibError> {
    let password = values.get(&database.password_secret).ok_or_else(|| {
        fail(format!(
            "The database user {}'s password is not made by this provision: its kind names no {} secret",
            database.user, database.password_secret
        ))
    })?;
    let admin = admin_of(database, made_admins)?;
    report.extend(create_on(database, &admin, password, &step.id())?);
    // a server made by this provision goes with the rest on a failure; one made before keeps nothing of this one
    if !database.made_here {
        made.push(Made::Database {
            use_: Box::new(database.clone()),
            admin,
        });
    }
    Ok(())
}

/// `create#Database`: the user and the database on the server, from a one-shot on its network or over verified TLS;
/// refused, naming what was found, when either is there. The report's lines.
pub(crate) fn create_on(
    database: &DatabaseUse,
    admin: &AdminLogin,
    password: &str,
    for_instance: &str,
) -> Result<Vec<String>, LibError> {
    let mut report = Vec::new();
    match &database.reach {
        Reach::Network {
            host,
            network,
            image,
            server,
            port,
        } => {
            let shot = database::one_shot_make(
                Server {
                    conf_name: &database.conf_name,
                    host: server,
                    port,
                },
                admin,
                &database.database,
                &database.user,
                password,
            )?;
            let (exit, lines) = run_one_shot(host, network, image, for_instance, shot)?;
            report.extend(lines.iter().cloned());
            if exit == EXIT_EXISTS {
                return Err(fail(format!(
                    "On {}: {}; a provision makes its own and does not take one that is there (reasonCode \
                     databaseExists)",
                    database.on,
                    lines.join("; ").replace("moqui: ", "")
                )));
            }
            if exit != 0 {
                return Err(fail(format!(
                    "Making database {} and user {} on {} failed (exit {exit}): {}",
                    database.database,
                    database.user,
                    database.on,
                    lines.join("; ")
                )));
            }
        }
        Reach::Direct { host, port } => {
            database::create_direct(
                &database::Direct {
                    conf_name: &database.conf_name,
                    host,
                    port,
                    admin,
                },
                &database.database,
                &database.user,
                password,
            )?;
            report.push(format!(
                "made database {} and user {} on {}",
                database.database, database.user, database.on
            ));
        }
    }
    Ok(report)
}

/// Drops a database and user this provision made, under the lease it ran under.
pub(crate) fn drop_on(database: &DatabaseUse, admin: &AdminLogin) -> Result<(), LibError> {
    match &database.reach {
        Reach::Network {
            host,
            network,
            image,
            server,
            port,
        } => {
            let shot = database::one_shot_drop(
                Server {
                    conf_name: &database.conf_name,
                    host: server,
                    port,
                },
                admin,
                &database.database,
                &database.user,
            )?;
            let (exit, lines) = run_one_shot(host, network, image, server, shot)?;
            if exit != 0 {
                return Err(fail(format!("exit {exit}: {}", lines.join("; "))));
            }
            Ok(())
        }
        Reach::Direct { host, port } => database::drop_direct(
            &database::Direct {
                conf_name: &database.conf_name,
                host,
                port,
                admin,
            },
            &database.database,
            &database.user,
        ),
    }
}

/// `check#Database`: whether the admin login connects, and whether the database and the user are there.
pub(crate) fn check_on(
    database: &DatabaseUse,
    admin: &AdminLogin,
    for_instance: &str,
) -> Result<database::DatabaseFound, LibError> {
    match &database.reach {
        Reach::Network {
            host,
            network,
            image,
            server,
            port,
        } => {
            let shot = database::one_shot_check(
                Server {
                    conf_name: &database.conf_name,
                    host: server,
                    port,
                },
                admin,
                &database.database,
                &database.user,
            )?;
            let (exit, lines) = run_one_shot(host, network, image, for_instance, shot)?;
            let count = |what: &str| {
                lines
                    .iter()
                    .find_map(|line| line.strip_prefix(&format!("moqui: {what} ")))
                    .is_some_and(|count| count.trim() != "0")
            };
            Ok(database::DatabaseFound {
                connected: exit == 0,
                database_exists: count("database"),
                user_exists: count("user"),
            })
        }
        Reach::Direct { host, port } => database::check_direct(
            &database::Direct {
                conf_name: &database.conf_name,
                host,
                port,
                admin,
            },
            &database.database,
            &database.user,
        ),
    }
}

/// Runs a one-shot client container on a database server's network to its end, and removes it: its exit code and the
/// `moqui:` lines of its logs (nothing else of them is kept).
pub(crate) fn run_one_shot(
    host_row: &EntityValue,
    network: &str,
    image: &str,
    for_instance: &str,
    shot: OneShot,
) -> Result<(i64, Vec<String>), LibError> {
    let host = connect_host(host_row)?;
    let mut tag = [0_u8; 4];
    moqui_vault::rng::Rng::fill(&mut moqui_vault::rng::OsRng, &mut tag);
    let name = format!("{for_instance}-db-{}", moqui_vault::text::hex(&tag));
    let spec = ContainerSpec {
        name: name.clone(),
        image: image.to_string(),
        pull: true,
        entrypoint: shot.entrypoint,
        cmd: shot.cmd,
        labels: vec![
            (ONE_SHOT_LABEL.to_string(), "true".to_string()),
            ("moqui.instance".to_string(), for_instance.to_string()),
        ],
        network: Some(network.to_string()).filter(|network| !network.is_empty()),
        restart: RestartPolicy::No,
        secrets: shot
            .files
            .into_iter()
            .map(|(file, bytes)| SecretFile::new(file, bytes, 0, 0))
            .collect(),
        ..ContainerSpec::default()
    };
    let container = host.create_spec(&spec)?;
    let ran = (|| -> Result<(i64, Vec<String>), LibError> {
        host.start(&container)?;
        let until = Instant::now() + ONE_SHOT_WAIT;
        let info = loop {
            let info = host
                .inspect(&container)?
                .ok_or_else(|| fail(format!("the one-shot {name} is gone")))?;
            if !info.running && info.status != "created" {
                break info;
            }
            if Instant::now() > until {
                return Err(fail(format!(
                    "the one-shot {name} did not end in {}s",
                    ONE_SHOT_WAIT.as_secs()
                )));
            }
            std::thread::sleep(Duration::from_millis(500));
        };
        let logs = host.logs(&container, 200).unwrap_or_default();
        let lines = logs
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("moqui:"))
            .map(str::to_string)
            .collect();
        Ok((info.exit_code.unwrap_or(-1), lines))
    })();
    if let Err(err) = host.remove_container(&container, true) {
        tracing::error!("Instances: the one-shot {name} was not removed: {err}");
    }
    ran
}

/// Pulls an image with the registry login of the lease (`X-Registry-Auth`, which the engine passes to the registry),
/// after checking that the item's own registry is the image's.
fn pull_with_login(host: &DockerHost, image: &EntityValue) -> Result<(), LibError> {
    use base64::Engine as _;
    let image_id = field(image, "instanceImageId");
    let binding = format!("InstanceImage:{image_id}:registry");
    plan::check_registry(image)?;
    let registry = plan::registry_name(&leased(&binding, "registry").unwrap_or_default());
    let location = plan::registry_name(&field(image, "registryLocation"));
    if registry != location {
        return Err(fail(format!(
            "{binding} is a login of {registry:?}, not {location}, which the image pulls from: the login goes only to \
             its own registry (reasonCode destinationMismatch)"
        )));
    }
    let user = leased(&binding, "username")?;
    let password = leased(&binding, "password")?;
    let auth = Zeroizing::new(
        json!({ "username": user.as_str(), "password": password.as_str(), "serveraddress": location }).to_string(),
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

/// Whether an engine's refusal says the network is there already.
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
    made: &mut Vec<Made>,
) -> Result<(), LibError> {
    let id = step.id();
    let host = connect_host(&step.host)?;
    let host_id = field(&step.host, "instanceHostId");
    let image = field(&step.image, "imageName");
    // the image first: a pairing token made before a long pull would spend its minutes waiting
    if field(&step.image, "registryLogin") == "Y" {
        pull_with_login(&host, &step.image)?;
    } else if host.image_volumes(&image)?.is_none() {
        host.pull(&image)?;
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
        host.create_volume(&name, &labels)?;
        made.push(Made::Volume {
            host: step.host.clone(),
            name: name.clone(),
        });
        report.push(format!("volume {name} on host {host_id}"));
        volumes.push((field(&volume, "mountPoint"), name));
    }
    let mut env: Vec<(String, String)> = rows(facade, INSTANCE_ENV, "appInstanceId", &id)?
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
            None => {
                let field_name = Some(field(&secret, "fieldName"))
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| "password".to_string());
                leased(&field(&secret, "bindingKey"), &field_name)?
            }
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
        // its Monitor identity (§4.8): its instance id, environment and the manager's OTLP address, settings too
        env.extend(crate::monitor::identity_env(facade, plan, step));
        // where it dials and the key it pins (§4.4): settings, not secrets
        if let Some((url, pin)) = crate::link::manager() {
            env.push(("moqui_conf__manage__url".to_string(), url));
            env.push(("moqui_conf__manage__pin".to_string(), pin));
        } else {
            report.push(format!(
                "AppInstance {id}: this manager's link has no address (manage@link-url), so the instance does not \
                 dial it; pair it with a code from System › Instances"
            ));
        }
        // its first start makes its seal with the environment's method (§4.5, M6): an automatic one (the manager's
        // transit, the default, or tpm) by itself; passphrase and shamir wait for `moqui operator init` on its socket
        match crate::transit::environment_method(facade, &step.instance)?.as_str() {
            method @ ("transit" | "tpm") => env.push(("moqui_conf__seal__method".to_string(), method.to_string())),
            method => report.push(format!(
                "AppInstance {id}: its environment's unseal method is {method}: its first start waits sealed for \
                 `moqui operator init` on its operator socket"
            )),
        }
        // its bootstrap secrets wait in memory for its link, sealed to its key when it takes them (§4.5)
        let mut bundle = crate::link::BundleValues::new();
        if let Some(password) = values.get("dbPassword") {
            bundle.insert(
                moqui_core::seal::secrets::DATASOURCE_PASSWORD.to_string(),
                password.clone(),
            );
        }
        crate::link::hold_bundle(&id, bundle, expires);
    }
    let port = |name: &str, row: &EntityValue| field(row, name).parse::<u16>().ok();
    let spec = kinds::container_spec(ContainerInput {
        kind: field(&step.kind, "instanceKindId"),
        name: field(&step.instance, "instanceName"),
        image,
        pull: false,
        env,
        volumes,
        host_config,
        labels,
        network,
        container_port: port("containerPort", &step.kind),
        host_port: port("hostPort", &step.instance),
        host_ip: Some(PUBLISH_ON.to_string()),
        run_as: Some(field(&step.kind, "runAsUser")).filter(|run_as| !run_as.is_empty()),
        secrets,
    })?;
    let container = host.create_spec(&spec)?;
    made.push(Made::Container {
        host: step.host.clone(),
        instance_id: id.clone(),
        container: container.clone(),
    });
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
