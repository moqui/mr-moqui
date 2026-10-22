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

//! `backup#EmbeddedDatabase`. The host takes the local archive. This service mirrors it over SFTP when
//! the named [`SystemMessageRemote`](https://moqui.org) has a `sendUrl`.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{basic_convert, is_empty, to_plain_string, Value};

use super::util::param_text;
use crate::backup::{self, BackupMode, BackupRequest};
use crate::context::ExecutionContext;
use crate::entity::{EntityFacade, EntityValue};
use crate::sftp::SftpClient;

const REMOTE: &str = "moqui.service.message.SystemMessageRemote";
const MESSAGE_TYPE: &str = "moqui.service.message.SystemMessageType";

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, backup_embedded_database)?;
    Ok(())
}

/// Take a live backup of each embedded cluster this process runs, rotate the local archives, and
/// mirror the keep set when `systemMessageRemoteId` names a remote with a `sendUrl`.
pub(super) fn backup_embedded_database(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    // a backup writes files and may upload them: nothing a read or a preview may do
    crate::entity::fence::check_effect(crate::entity::fence::Effect::Resource, "backup#EmbeddedDatabase")?;
    let Some(runner) = facade.backup_runner() else {
        tracing::info!("embedded database backup skipped, this process has no embedded cluster");
        return Ok(outcome_map(
            &backup::BackupOutcome {
                skipped: true,
                backup_dir: Path::new("").to_path_buf(),
                archive_path: Path::new("").to_path_buf(),
                backup_kind: String::new(),
                bytes_written: 0,
                promoted: Vec::new(),
                removed: Vec::new(),
                keep: Vec::new(),
                renames: Vec::new(),
                errors: Vec::new(),
            },
            "N",
        ));
    };
    let request = request_from(parameters);
    let mut outcome = runner(&request)?;
    let remote_id = param_text(parameters, "systemMessageRemoteId");
    let mirrored = if remote_id.is_empty() {
        "N".to_string()
    } else {
        match mirror(facade, &remote_id, &outcome) {
            Ok(flag) => flag,
            Err(err) => {
                outcome.errors.push(err.to_string());
                "N".to_string()
            }
        }
    };
    for error in &outcome.errors {
        add_error(error.clone());
    }
    Ok(outcome_map(&outcome, &mirrored))
}

fn request_from(parameters: &BTreeMap<String, Value>) -> BackupRequest {
    let mode = if param_text(parameters, "backupMode").eq_ignore_ascii_case("incremental") {
        BackupMode::Incremental
    } else {
        BackupMode::Dump
    };
    let dir = param_text(parameters, "backupDir");
    let minutes = count_param(parameters, "timeoutMinutes", 120).max(1);
    BackupRequest {
        mode,
        backup_dir: (!dir.is_empty()).then(|| Path::new(&dir).to_path_buf()),
        daily: count_param(parameters, "dailyCount", 7),
        weekly: count_param(parameters, "weeklyCount", 5),
        monthly: count_param(parameters, "monthlyCount", 12),
        yearly: optional_count(parameters, "yearlyCount"),
        full_every: count_param(parameters, "fullEvery", 7),
        fast_checkpoint: parameters
            .get("fastCheckpoint")
            .is_some_and(|value| matches!(basic_convert(value, "Boolean").value, Value::Bool(true))),
        timeout: Duration::from_secs(u64::from(minutes) * 60),
    }
}

fn mirror(facade: &EntityFacade, remote_id: &str, outcome: &backup::BackupOutcome) -> Result<String, LibError> {
    let Some(remote) = find_one(facade, REMOTE, "systemMessageRemoteId", remote_id)? else {
        return Err(LibError::new(format!("no SystemMessageRemote with id {remote_id}")));
    };
    let send_url = field(&remote, "sendUrl");
    if send_url.is_empty() {
        tracing::info!("embedded database backup mirror is off, SystemMessageRemote {remote_id} has no sendUrl");
        return Ok("N".to_string());
    }
    if field(&remote, "username").is_empty() {
        return Err(LibError::new(format!(
            "SystemMessageRemote {remote_id} has a sendUrl and no username"
        )));
    }
    let private_key = field(&remote, "privateKey");
    let public_key = field(&remote, "publicKey");
    if private_key.is_empty() && field(&remote, "password").is_empty() {
        return Err(LibError::new(format!(
            "SystemMessageRemote {remote_id} needs a password or a private key"
        )));
    }
    if !private_key.is_empty() && public_key.is_empty() {
        return Err(LibError::new(format!(
            "SystemMessageRemote {remote_id} has a private key and no public key"
        )));
    }
    let type_id = field(&remote, "systemMessageTypeId");
    let Some(message_type) = find_one(facade, MESSAGE_TYPE, "systemMessageTypeId", &type_id)? else {
        return Err(LibError::new(format!(
            "SystemMessageRemote {remote_id} has no SystemMessageType"
        )));
    };
    let send_path = field(&message_type, "sendPath");
    if send_path.is_empty() {
        return Err(LibError::new(format!(
            "SystemMessageType {type_id} has no sendPath for the backup mirror"
        )));
    }
    let mut client = client_for(&remote, &send_url, &private_key, &public_key)?;
    if field(&remote, "remoteAttributes") == "N" {
        client.preserve_attributes(false);
    }
    client.connect()?;
    let mirrored = mirror_with(&mut client, &send_path, outcome);
    client.close();
    mirrored
}

fn mirror_with(client: &mut SftpClient, send_path: &str, outcome: &backup::BackupOutcome) -> Result<String, LibError> {
    client.mkdirs(send_path)?;
    let listed = match client.ls(send_path) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::error!("backup mirror could not list {send_path}: {err}");
            return Err(err);
        }
    };
    let remote_names: Vec<String> = listed
        .iter()
        .map(|(path, _)| path.rsplit('/').next().unwrap_or(path).to_string())
        .collect();
    let keep_names: Vec<String> = outcome.keep.iter().map(|kept| kept.name.clone()).collect();
    let plan = backup::mirror_plan(&keep_names, &remote_names, &outcome.renames, backup::is_archive_name);
    let mut failed = false;
    for (old, new) in &plan.renames {
        let from = child(send_path, old);
        let to = child(send_path, new);
        if let Err(err) = client.rename(&from, &to) {
            tracing::error!("backup mirror could not rename {from} to {to}: {err}");
            failed = true;
        }
    }
    for name in &plan.uploads {
        let Some(kept) = outcome.keep.iter().find(|kept| &kept.name == name) else {
            continue;
        };
        let remote = child(send_path, name);
        let put = if kept.path.is_dir() {
            client.put_tree(&remote, &kept.path)
        } else {
            client.put_file(&remote, &kept.path)
        };
        if let Err(err) = put {
            tracing::error!("backup mirror could not upload {name}: {err}");
            failed = true;
        }
    }
    for name in &plan.deletes {
        let remote = child(send_path, name);
        let removed = match client.stat(&remote) {
            Ok(Some(found)) if found.kind == crate::sftp::SftpKind::Directory => client.rm_tree(&remote),
            Ok(Some(_)) => client.rm(&remote),
            Ok(None) => Ok(()),
            Err(err) => Err(err),
        };
        if let Err(err) = removed {
            tracing::error!("backup mirror could not delete {remote}: {err}");
            failed = true;
        }
    }
    if failed {
        Err(LibError::new(
            "embedded database backup mirror did not finish; the local archive is in place",
        ))
    } else {
        Ok("Y".to_string())
    }
}

fn client_for(remote: &EntityValue, host: &str, private_key: &str, public_key: &str) -> Result<SftpClient, LibError> {
    let mut client = SftpClient::new(host, &field(remote, "username"), None)?;
    if private_key.is_empty() {
        client.password(&field(remote, "password"));
    } else {
        client.public_key_pkcs8(private_key, Some(public_key));
    }
    client.host_key_fingerprint(&field(remote, "sshHostKeyFingerprint"));
    Ok(client)
}

fn child(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

fn outcome_map(outcome: &backup::BackupOutcome, mirrored: &str) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    out.insert(
        "archivePath".to_string(),
        Value::string(outcome.archive_path.display().to_string()),
    );
    out.insert("backupKind".to_string(), Value::string(&outcome.backup_kind));
    out.insert(
        "bytesWritten".to_string(),
        Value::I64(i64::try_from(outcome.bytes_written).unwrap_or(i64::MAX)),
    );
    out.insert("promoted".to_string(), Value::string(outcome.promoted.join(", ")));
    out.insert("archivesRemoved".to_string(), Value::string(outcome.removed.join(", ")));
    out.insert("mirrored".to_string(), Value::string(mirrored));
    out.insert(
        "skipped".to_string(),
        Value::string(if outcome.skipped { "Y" } else { "N" }),
    );
    out
}

fn count_param(parameters: &BTreeMap<String, Value>, name: &str, default: u32) -> u32 {
    match parameters.get(name) {
        None => default,
        Some(value) if is_empty(value) => default,
        Some(value) => match basic_convert(value, "Long").value {
            Value::I64(number) if number > 0 => u32::try_from(number).unwrap_or(u32::MAX),
            Value::I64(0) => 0,
            Value::I64(_) => 0,
            _ => default,
        },
    }
}

fn optional_count(parameters: &BTreeMap<String, Value>, name: &str) -> Option<u32> {
    match parameters.get(name) {
        None => None,
        Some(value) if is_empty(value) => None,
        Some(value) => match basic_convert(value, "Long").value {
            Value::I64(number) if number > 0 => Some(u32::try_from(number).unwrap_or(u32::MAX)),
            Value::I64(_) => Some(0),
            _ => None,
        },
    }
}

fn add_error(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_error(text));
}

fn find_one(
    facade: &EntityFacade,
    entity: &str,
    field_name: &str,
    value: &str,
) -> Result<Option<EntityValue>, LibError> {
    if value.is_empty() {
        return Ok(None);
    }
    facade
        .find(entity)?
        .disable_authz()
        .eq(field_name, Value::string(value))
        .one()
}

fn field(row: &EntityValue, name: &str) -> String {
    match row.get(name) {
        Some(value) if !is_empty(value) => to_plain_string(value),
        _ => String::new(),
    }
}
