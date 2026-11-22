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

//! The environment (plans/agent-code/13-Servers-Linking.md section 1): one MR Moqui instance, one node or a cluster,
//! is one environment. Its id is made once and shared by the cluster through the `AgentCodeSetting` row
//! `environment.id`; `runtime/db/agent_code_environment_id` caches it on each node. The descriptor
//! (`GET /.well-known/moqui/agent-code`, 09 section 6) tells a client what this server is and what it can do, with no
//! user data.

use std::path::Path;

use moqui_core::entity::EntityFacade;
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde_json::{json, Map};

use crate::settings::SETTING;
use crate::support::text_of;

/// The internal setting rows (not in the schema, so a client cannot write them).
pub const ID_SETTING: &str = "environment.id";
pub const LABEL_SETTING: &str = "environment.label";

const ID_FILE: &str = "db/agent_code_environment_id";

fn valid(id: &str) -> bool {
    id.starts_with("env_") && id.len() <= 40 && id[4..].bytes().all(|b| b.is_ascii_alphanumeric())
}

fn new_id() -> Result<String, Error> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|err| Error::new(format!("no random bytes for the environment id: {err}")))?;
    Ok(format!("env_{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()))
}

fn read_file(runtime: &Path) -> Option<String> {
    std::fs::read_to_string(runtime.join(ID_FILE))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|id| valid(id))
}

/// Write the cache file with a temp file and a rename, so a crash leaves the old file or the new one.
fn write_file(runtime: &Path, id: &str) {
    let path = runtime.join(ID_FILE);
    let temp = path.with_extension("tmp");
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&temp, id))
        .and_then(|()| std::fs::rename(&temp, &path));
    if let Err(err) = written {
        tracing::warn!("agent code: the environment id cache {} was not written: {err}", path.display());
    }
}

/// The environment id: the shared row, else the cache file (a node that lost its database row keeps its id), else a
/// new one. The first node to store the row wins; the others read it.
///
/// # Errors
///
/// When the row can be neither read nor written.
pub fn environment_id(facade: &EntityFacade, runtime: &Path) -> Result<String, Error> {
    let read_row = || -> Result<Option<String>, Error> {
        Ok(facade
            .find(SETTING)?
            .eq("settingName", Value::string(ID_SETTING))
            .disable_authz()
            .one()?
            .and_then(|row| text_of(&row, "settingValue"))
            .filter(|id| valid(id)))
    };
    if let Some(id) = read_row()? {
        if read_file(runtime).as_deref() != Some(id.as_str()) {
            write_file(runtime, &id);
        }
        return Ok(id);
    }
    let id = match read_file(runtime) {
        Some(id) => id,
        None => new_id()?,
    };
    let mut row = facade.make_value(SETTING)?;
    row.set("settingName", Value::string(ID_SETTING));
    row.set("settingValue", Value::string(&id));
    if let Err(err) = row.create(facade) {
        // another node stored one first
        if let Some(stored) = read_row()? {
            write_file(runtime, &stored);
            return Ok(stored);
        }
        return Err(err);
    }
    write_file(runtime, &id);
    Ok(id)
}

/// The label: the `environment.label` row, else the host name.
#[must_use]
pub fn label(facade: &EntityFacade) -> String {
    let stored = facade
        .find(SETTING)
        .and_then(|find| find.eq("settingName", Value::string(LABEL_SETTING)).disable_authz().one())
        .ok()
        .flatten()
        .and_then(|row| text_of(&row, "settingValue"));
    stored.unwrap_or_else(host_name)
}

fn host_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "MR Moqui".to_string())
}

/// The T3 environment capabilities, by name, with what this server has built. A capability of a later phase answers
/// false until that phase ships.
#[must_use]
pub fn capabilities() -> Map<String, serde_json::Value> {
    const BUILT: &[&str] = &["commandReceipts", "shellStream", "settingsSchema", "moquiNativeEngine"];
    const LATER: &[&str] = &[
        "providerInstances",
        "threadTerminals",
        "worktrees",
        "checkpoints",
        "pullRequests",
        "pullRequestWatch",
        "sourceControlHosts",
        "delegatedTasks",
        "sideChats",
        "threadFork",
        "providerSwitch",
        "scheduledTasks",
        "webhooks",
        "notifications",
        "pushNotifications",
        "pairing",
        "linkedEnvironments",
        "sshLaunch",
        "previewBrowser",
        "previewTunnel",
        "deviceSupport",
        "secrets",
        "guestShare",
        "voiceInput",
        "usage",
        "t3Import",
        "moquiHosting",
        "edgeLink",
        "clusterNodes",
        "settingsSecretReferences",
    ];
    BUILT
        .iter()
        .map(|name| ((*name).to_string(), json!(true)))
        .chain(LATER.iter().map(|name| ((*name).to_string(), json!(false))))
        .collect()
}

/// The machine kind: `MOQUI_AGENT_CODE_MACHINE` (server, cloud, desktop, laptop), else server.
fn machine() -> String {
    std::env::var("MOQUI_AGENT_CODE_MACHINE")
        .ok()
        .filter(|kind| matches!(kind.as_str(), "server" | "cloud" | "desktop" | "laptop"))
        .unwrap_or_else(|| "server".to_string())
}

fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

/// The descriptor of this environment.
///
/// # Errors
///
/// When the environment id cannot be had.
pub fn descriptor(facade: &EntityFacade, runtime: &Path) -> Result<serde_json::Value, Error> {
    Ok(json!({
        "environmentId": environment_id(facade, runtime)?,
        "label": label(facade),
        "platform": {"os": std::env::consts::OS, "arch": arch(), "machine": machine()},
        "serverVersion": env!("CARGO_PKG_VERSION"),
        "protocol": crate::PROTOCOL,
        "databaseMigration": null,
        "capabilities": capabilities(),
        "serverInstallation": {"kind": "moqui-package"},
        "endpoints": [],
    }))
}

#[cfg(test)]
#[path = "environment_tests.rs"]
mod tests;
