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

//! The instance kinds (plans/Vault-Instances.md §4.1, §4.2): `mr-moqui`, `postgres`, `mysql` and `opensearch`. A kind's
//! rows (`InstanceKind` with its env, volume, host config, secret and dependency rows, framework/data/
//! InstanceTypeData.xml) say what its container is made of; this module turns an instance's rows into the container
//! that stays ([`ContainerSpec`]), with each secret a file under `/run/secrets` and the image's `*_FILE` setting naming
//! it. What a database kind does on a server is [`crate::database`].
//!
//! OpenSearch reads no `*_FILE` setting, so its entrypoint is wrapped: a shell reads the admin password from its file
//! into the setting the image's demo security configuration takes and starts the image's entrypoint. The value is in
//! no container's environment, labels or command (`docker inspect` shows none), though the OpenSearch process is
//! started with it in its own environment, as the image requires.

use std::collections::BTreeMap;
use std::time::Duration;

use moqui_lib::docker::{
    ContainerSpec, HealthCheck, Mount, MountKind, PortBinding, RestartPolicy, SecretFile, SECRETS_DIR,
};
use moqui_lib::error::Error as LibError;

use zeroize::Zeroizing;

use crate::common::fail;

/// The file an mr-moqui instance's one-time pairing token is given as (§4.2, §4.4); the manager link (M2) reads it.
pub const PAIRING_TOKEN_FILE: &str = "moqui-pairing-token";
/// How long a pairing token works (§4.2): 15 minutes.
pub const PAIRING_TOKEN_MINUTES: i64 = 15;
/// The placeholders a kind's env and volume templates may hold (`${name}`); nothing else is expanded.
pub const PLACEHOLDERS: &[&str] = &[
    "appInstanceId",
    "instanceName",
    "hostName",
    "databaseConf",
    "databaseHost",
    "databasePort",
    "databaseName",
    "databaseUser",
    "searchBackend",
    "searchUrl",
    "secretsDir",
];

/// Fills a template's `${name}` placeholders from `values`; a placeholder that is not one of [`PLACEHOLDERS`] is
/// refused, so no template runs anything (the Java templates were Groovy).
pub fn expand(template: &str, values: &BTreeMap<&str, String>) -> Result<String, LibError> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some((before, after)) = rest.split_once("${") {
        out.push_str(before);
        let Some((name, tail)) = after.split_once('}') else {
            return Err(fail(format!("The template {template:?} has a ${{ with no }}")));
        };
        if !PLACEHOLDERS.contains(&name) {
            return Err(fail(format!(
                "The template {template:?} names ${{{name}}}, which is not one of {}",
                PLACEHOLDERS.join(", ")
            )));
        }
        out.push_str(values.get(name).map_or("", String::as_str));
        rest = tail;
    }
    out.push_str(rest);
    Ok(out)
}

/// A credential for a secret of `kind`: [`crate::common::new_credential`], with what OpenSearch's password rule asks
/// beside (an upper and a lower case letter, a digit and another character).
pub fn credential_for(kind: &str) -> Zeroizing<String> {
    let mut credential = crate::common::new_credential();
    if kind == "opensearch" {
        credential.push_str("-aA1");
    }
    credential
}

/// What an instance's container is made of: its rows, read by the provision.
#[derive(Debug, Default)]
pub struct ContainerInput {
    pub kind: String,
    pub name: String,
    pub image: String,
    pub pull: bool,
    pub env: Vec<(String, String)>,
    /// Each mount point with its volume's name.
    pub volumes: Vec<(String, String)>,
    /// The host config rows, by name: RestartPolicy, HealthCmd, User.
    pub host_config: BTreeMap<String, String>,
    pub labels: Vec<(String, String)>,
    pub network: Option<String>,
    pub container_port: Option<u16>,
    pub host_port: Option<u16>,
    /// The host's address a published port listens on (loopback for a host that is this machine).
    pub host_ip: Option<String>,
    /// `uid:gid` the secret files are owned by.
    pub run_as: Option<String>,
    /// The secret files: each file's name, the image's setting that names it (if any), and its value.
    pub secrets: Vec<(String, Option<String>, Zeroizing<Vec<u8>>)>,
}

/// The container that stays for `input`: settings that are not secret, each secret a file owned by the kind's user
/// (0400) and named by its `*_FILE` setting, the volumes, the environment's network, the published port, the restart
/// policy and health check. [`ContainerSpec::check`] refuses it, before anything is sent, when a secret's value is in
/// its environment, labels, command or entrypoint.
pub fn container_spec(input: ContainerInput) -> Result<ContainerSpec, LibError> {
    let (uid, gid) = match input.run_as.as_deref().and_then(|run_as| run_as.split_once(':')) {
        Some((uid, gid)) => (
            uid.trim()
                .parse()
                .map_err(|_| fail(format!("runAsUser {uid}:{gid} is not uid:gid")))?,
            gid.trim()
                .parse()
                .map_err(|_| fail(format!("runAsUser {uid}:{gid} is not uid:gid")))?,
        ),
        None => (0, 0),
    };
    let mut env = input.env;
    let mut secrets = Vec::with_capacity(input.secrets.len());
    for (file, setting, bytes) in input.secrets {
        let path = format!("{SECRETS_DIR}/{file}");
        if let Some(setting) = setting.filter(|setting| !setting.is_empty()) {
            env.retain(|(name, _)| *name != setting);
            env.push((setting, path));
        }
        secrets.push(SecretFile::new(file, bytes, uid, gid));
    }
    let mounts = input
        .volumes
        .into_iter()
        .map(|(target, source)| Mount {
            kind: MountKind::Volume,
            source,
            target,
            read_only: false,
        })
        .collect();
    let restart = match input.host_config.get("RestartPolicy").map(String::as_str) {
        None | Some("" | "no") => RestartPolicy::No,
        Some("always") => RestartPolicy::Always,
        Some("unless-stopped") => RestartPolicy::UnlessStopped,
        Some("on-failure") => RestartPolicy::OnFailure(5),
        Some(other) => {
            return Err(fail(format!(
                "RestartPolicy {other} is not no, always, unless-stopped or on-failure"
            )))
        }
    };
    let health = input
        .host_config
        .get("HealthCmd")
        .filter(|command| !command.trim().is_empty())
        .map(|command| HealthCheck {
            test: vec!["CMD-SHELL".to_string(), command.clone()],
            interval: Some(Duration::from_secs(10)),
            timeout: Some(Duration::from_secs(5)),
            retries: Some(12),
            start_period: Some(Duration::from_secs(30)),
        });
    let ports = match (input.container_port, input.host_port) {
        (Some(container_port), Some(host_port)) => vec![PortBinding {
            container_port,
            protocol: "tcp".to_string(),
            host_port: Some(host_port),
            host_ip: input.host_ip,
        }],
        _ => Vec::new(),
    };
    let entrypoint = if input.kind == "opensearch" {
        opensearch_entrypoint(&secrets)
    } else {
        Vec::new()
    };
    Ok(ContainerSpec {
        name: input.name,
        image: input.image,
        pull: input.pull,
        entrypoint,
        env,
        labels: input.labels,
        mounts,
        network: input.network,
        ports,
        restart,
        health,
        user: input.host_config.get("User").filter(|user| !user.is_empty()).cloned(),
        secrets,
        ..ContainerSpec::default()
    })
}

/// OpenSearch's wrapped entrypoint: its admin password read from its file into `OPENSEARCH_INITIAL_ADMIN_PASSWORD`,
/// then the image's own entrypoint. The command names the file, never the value.
fn opensearch_entrypoint(secrets: &[SecretFile]) -> Vec<String> {
    let Some(file) = secrets.iter().find(|secret| secret.name == "opensearch-admin-password") else {
        return Vec::new();
    };
    vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        format!(
            "OPENSEARCH_INITIAL_ADMIN_PASSWORD=\"$(cat {})\"; export OPENSEARCH_INITIAL_ADMIN_PASSWORD; \
             exec /usr/share/opensearch/opensearch-docker-entrypoint.sh opensearch",
            file.path()
        ),
    ]
}
