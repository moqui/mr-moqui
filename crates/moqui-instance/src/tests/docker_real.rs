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

//! Opt-in, against a real Docker or Podman socket (plans/archive/Vault-Instances.md §6 M1, §7): a Postgres instance provisioned
//! with its admin password as a file, checked by a health check that signs in over TCP with that file (so the
//! password the server made, deposited and never read back is the one Postgres took); a second instance's database
//! and user (a SCRAM verifier) made on it by the one-shot client on their network; then a rotation (M5) of the user's
//! password and of the admin's, each set at the source by a one-shot and signed in with before its new revision is
//! deposited; both removed with their volumes.
//! Run by name, with the socket named:
//!
//! `MOQUI_CI_DOCKER_SOCKET=$HOME/.docker/run/docker.sock cargo test -p moqui-instance docker_real -- --ignored`

use std::time::{Duration, Instant};

use moqui_lib::docker::DockerHost;
use moqui_lib::object::Value;
use serde_json::json;

use super::{create, field_of, load, login, setup, sys, vault};
use crate::services::{create_instance, NewInstance};

#[test]
#[ignore = "opt-in: a real Docker or Podman socket in MOQUI_CI_DOCKER_SOCKET, and an image pull"]
fn a_postgres_instance_on_a_real_engine_takes_its_password_from_a_file() {
    let Some(socket) = std::env::var_os("MOQUI_CI_DOCKER_SOCKET") else {
        eprintln!("MOQUI_CI_DOCKER_SOCKET is not set: skipped");
        return;
    };
    let socket = socket.to_string_lossy().to_string();
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    load(&facade, "InstanceTypeData.xml");
    load(&facade, "VaultTypeData.xml");
    let mut vault = vault(&facade, "real");
    let tag = format!("{}", std::process::id());
    let environment = format!("realenv{tag}");
    create(
        &facade,
        "moqui.server.instance.InstanceEnvironment",
        json!({ "environmentId": environment, "vaultId": vault.vault_id }),
    );
    create(
        &facade,
        "moqui.server.instance.InstanceHost",
        json!({ "instanceHostId": "real-socket", "hostTypeId": "docker", "hostProtocol": "unix", "hostAddress": socket }),
    );
    let pg = sys(|| {
        create_instance(
            &facade,
            &NewInstance {
                instance_image_id: "Postgres18".to_string(),
                instance_host_id: "real-socket".to_string(),
                environment_id: environment.clone(),
                instance_name: format!("moqui-it-pg-{tag}"),
                ..NewInstance::default()
            },
        )
    })
    .unwrap();
    // the check signs in over TCP to the container's own address (not loopback, which the image trusts) with the file
    sys(|| {
        let mut row = facade
            .find("moqui.server.instance.AppInstanceHostConfig")
            .unwrap()
            .eq("appInstanceId", Value::string(&pg))
            .eq("hostConfigName", Value::string("HealthCmd"))
            .one()
            .unwrap()
            .unwrap();
        row.set(
            "hostConfigValue",
            Value::string(
                "PGPASSWORD=\"$(cat /run/secrets/postgres-password)\" psql -h \"$(hostname -i | cut -d' ' -f1)\" \
                 -U postgres -c 'select 1' >/dev/null",
            ),
        );
        row.update(&facade).unwrap();
    });
    // a second instance whose database is made on the first by the one-shot on their network: for this test only, the
    // postgres kind takes a postgres database and binds its user's password
    create(
        &facade,
        "moqui.server.instance.InstanceKindDependency",
        json!({ "instanceKindId": "postgres", "dependencyRole": "database", "allowedKindId": "postgres", "optional": "Y" }),
    );
    create(
        &facade,
        "moqui.server.instance.InstanceKindSecret",
        json!({ "instanceKindId": "postgres", "secretName": "dbPassword", "vaultItemTypeId": "database-credential",
                "bindingPattern": "AppInstance:{id}:dbPassword", "fieldName": "password", "generated": "Y",
                "whenDependency": "database", "rotateService": crate::rotate::ROTATE_SERVICE }),
    );
    let user = sys(|| {
        create_instance(
            &facade,
            &NewInstance {
                instance_image_id: "Postgres18".to_string(),
                instance_host_id: "real-socket".to_string(),
                environment_id: environment.clone(),
                instance_name: format!("moqui-it-user-{tag}"),
                database_instance_id: pg.clone(),
                ..NewInstance::default()
            },
        )
    })
    .unwrap();
    login(&vault.maker.user);
    let registry = moqui_vault_server::lease::registry(ctx.factory());
    let provisioned = crate::provision::provision(&facade, &registry, &user, None).map(|outcome| outcome.report);
    let container = field_of(
        &facade,
        "moqui.server.instance.AppInstance",
        "appInstanceId",
        &pg,
        "instanceUuid",
    );
    let host = DockerHost::unix(&socket);
    let checked = (|| -> Result<(), String> {
        let report = provisioned.map_err(|err| err.to_string())?;
        // the one-shot made the database and the user on the first instance, from its network
        let database = format!("moqui_it_user_{tag}");
        if !report
            .iter()
            .any(|line| line == &format!("moqui: made database {database} and user {database}"))
        {
            return Err(format!("{report:?}"));
        }
        let until = Instant::now() + Duration::from_secs(120);
        loop {
            let info = host
                .inspect(&container)
                .map_err(|err| err.to_string())?
                .ok_or("the container is gone")?;
            match info.health.as_deref() {
                Some("healthy") => break,
                Some("unhealthy") => return Err(host.logs(&container, 50).unwrap_or_default()),
                _ if Instant::now() > until => return Err(format!("not healthy in time: {info:?}")),
                _ => std::thread::sleep(Duration::from_secs(2)),
            }
        }
        // what docker inspect shows names the file, and holds no password setting
        let inspected = host
            .send(
                moqui_lib::http::Method::Get,
                &format!("/containers/{container}/json"),
                None,
                Duration::from_secs(30),
            )
            .map_err(|err| err.to_string())?;
        let body: serde_json::Value = serde_json::from_str(inspected.body()).map_err(|err| err.to_string())?;
        let env: Vec<String> = body["Config"]["Env"]
            .as_array()
            .map(|env| {
                env.iter()
                    .filter_map(|entry| entry.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if !env.contains(&"POSTGRES_PASSWORD_FILE=/run/secrets/postgres-password".to_string()) {
            return Err(format!("{env:?}"));
        }
        if env.iter().any(|entry| entry.starts_with("POSTGRES_PASSWORD=")) {
            return Err(format!("{env:?}"));
        }
        Ok(())
    })();
    // M5: the user's password, then the admin's, rotated at the source by one-shots on their network and signed in with
    let rotated = checked.as_ref().map_err(String::clone).and_then(|()| {
        let bindings = [
            format!("AppInstance:{user}:dbPassword"),
            format!("AppInstance:{pg}:admin"),
        ];
        for binding in &bindings {
            super::rotate::take_in(&mut vault, &facade, binding);
        }
        let plan = sys(|| crate::rotate::plan(&facade, &environment, &bindings)).map_err(|err| err.to_string())?;
        if plan.credentials.len() != 2 {
            return Err(format!("{:?}", plan.not_rotated));
        }
        let lease_id = super::rotate::approved(&mut vault, &facade, &registry, &plan);
        let outcome = sys(|| {
            crate::rotate::run(
                &facade,
                &registry,
                &crate::rotate::LinkInstances { facade: &facade },
                &plan,
                Some(&lease_id),
                &crate::rotate::Options {
                    wait: Duration::from_secs(120),
                },
            )
        })
        .map_err(|err| err.to_string())?;
        if !outcome.failed.is_empty() || outcome.rotated.len() != 2 {
            return Err(format!("{:?}\n{}", outcome.failed, outcome.report.join("\n")));
        }
        Ok(())
    });
    // removed whatever the check found, with their volumes and network
    let removed_user = sys(|| crate::lifecycle::act(&facade, &registry, &user, "remove", None, true));
    let removed = sys(|| crate::lifecycle::act(&facade, &registry, &pg, "remove", None, true));
    removed_user.unwrap();
    if let Err(err) = host.delete_network(&format!("moqui-{environment}")) {
        eprintln!("the network was not removed: {err}");
    }
    checked.unwrap();
    rotated.unwrap();
    removed.unwrap();
    assert!(host.inspect(&container).unwrap().is_none());
}
