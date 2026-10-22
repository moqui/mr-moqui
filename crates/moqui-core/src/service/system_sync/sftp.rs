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

//! `send#SystemMessageSftp` and `poll#SystemMessageSftp`, from the `moqui-sftp` component's Groovy scripts, on
//! [`crate::sftp::SftpClient`].

use std::collections::{BTreeMap, BTreeSet};

use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use super::{add_error, has_error, integer_of, millis_of, now_millis, param_text, stamp, text_of};
use crate::entity::{EntityFacade, EntityValue};
use crate::service::call::{sync_with, CallOptions};
use crate::sftp::SftpClient;

pub(super) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, send)?;
    rust_service!(map, poll)?;
    Ok(())
}

const MESSAGE: &str = "moqui.service.message.SystemMessage";
const MESSAGE_TYPE: &str = "moqui.service.message.SystemMessageType";
const REMOTE: &str = "moqui.service.message.SystemMessageRemote";
const RECEIVE: &str = "org.moqui.impl.SystemMessageServices.receive#IncomingSystemMessage";

/// `send#SystemMessageSftp`: put the message text at the expanded `sendPath`; the file name is the remote message ID,
/// else the message ID, and it becomes the `remoteMessageId`.
pub(super) fn send(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    refuse_outside(facade, "send#SystemMessageSftp")?;
    let message_id = param_text(parameters, "systemMessageId");
    let Some(message) = find_one(facade, MESSAGE, "systemMessageId", &message_id)? else {
        add_error(format!("Could not find SystemMessage with ID {message_id}"));
        return Ok(BTreeMap::new());
    };
    let type_id = field(&message, "systemMessageTypeId");
    let Some(message_type) = find_one(facade, MESSAGE_TYPE, "systemMessageTypeId", &type_id)? else {
        add_error(format!(
            "System message {message_id} has no SystemMessageType, not sending."
        ));
        return Ok(BTreeMap::new());
    };
    let remote_id = field(&message, "systemMessageRemoteId");
    let Some(remote) = find_one(facade, REMOTE, "systemMessageRemoteId", &remote_id)? else {
        add_error(format!(
            "System message {message_id} has no SystemMessageRemote, not sending."
        ));
        return Ok(BTreeMap::new());
    };
    if field(&remote, "sendUrl").is_empty() {
        add_error(format!("No sendUrl on SystemMessageRemote {remote_id}, not sending."));
        return Ok(BTreeMap::new());
    }
    if field(&remote, "username").is_empty() {
        add_error(format!("No username on SystemMessageRemote {remote_id}, not sending."));
        return Ok(BTreeMap::new());
    }

    let millis = millis_of(message.get("messageDate"))
        .or_else(|| millis_of(message.get("initDate")))
        .unwrap_or_else(now_millis);
    let message_date = stamp(millis);
    let remote_message_id = field(&message, "remoteMessageId");
    let names = [
        ("systemMessageId", Value::string(&message_id)),
        ("remoteMessageId", text_value(&remote_message_id)),
        ("systemMessageTypeId", Value::string(&type_id)),
        ("systemMessageRemoteId", Value::string(&remote_id)),
        ("date", Value::string(format_date(&message_date, "yyyy-MM-dd")?)),
        (
            "dateTime",
            Value::string(format_date(&message_date, "yyyy-MM-dd-HH-mm-ss")?),
        ),
    ];
    let Some(send_path) = expand_path(&message_type, "sendPath", &names)? else {
        return Ok(BTreeMap::new());
    };
    let filename = if remote_message_id.is_empty() {
        message_id.clone()
    } else {
        remote_message_id
    };
    let charset = field(&remote, "remoteCharset");

    let mut client = client_for(&remote, &field(&remote, "sendUrl"))?;
    if field(&remote, "remoteAttributes") == "N" {
        client.preserve_attributes(false);
    }
    tracing::info!(
        "SFTP put message {message_id} to path {send_path} file {filename} charset {}",
        if charset.is_empty() { "UTF-8" } else { &charset }
    );
    client.connect()?;
    let put = client.put_text(&send_path, &filename, &field(&message, "messageText"), &charset, false);
    client.close();
    put?;
    let mut out = BTreeMap::new();
    out.insert("remoteMessageId".to_string(), Value::string(filename));
    Ok(out)
}

/// `poll#SystemMessageSftp`: receive each file in the expanded `receivePath` that matches `receiveFilePattern`, in name
/// order up to `fileCountLimit`, then delete or move it as `receiveResponseEnumId` says. The first receive that fails
/// stops the poll.
pub(super) fn poll(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    refuse_outside(facade, "poll#SystemMessageSftp")?;
    let type_id = param_text(parameters, "systemMessageTypeId");
    let remote_id = param_text(parameters, "systemMessageRemoteId");
    let Some(message_type) = find_one(facade, MESSAGE_TYPE, "systemMessageTypeId", &type_id)? else {
        add_error(format!("Could not find SystemMessageType with ID {type_id}"));
        return Ok(BTreeMap::new());
    };
    let Some(remote) = find_one(facade, REMOTE, "systemMessageRemoteId", &remote_id)? else {
        add_error(format!("Could not find SystemMessageRemote with ID {remote_id}"));
        return Ok(BTreeMap::new());
    };
    let names = [
        ("systemMessageTypeId", Value::string(&type_id)),
        ("systemMessageRemoteId", Value::string(&remote_id)),
    ];
    let Some(receive_path) = expand_path(&message_type, "receivePath", &names)? else {
        return Ok(BTreeMap::new());
    };
    let Some(move_path) = expand_path(&message_type, "receiveMovePath", &names)? else {
        return Ok(BTreeMap::new());
    };
    let pattern = field(&message_type, "receiveFilePattern");
    let pattern = (!pattern.is_empty()).then_some(pattern);
    let response = field(&message_type, "receiveResponseEnumId");
    let charset = field(&remote, "remoteCharset");
    let limit = integer_of(parameters.get("fileCountLimit")).filter(|limit| *limit > 0);

    let mut host = field(&remote, "receiveUrl");
    if host.is_empty() {
        host = field(&remote, "sendUrl");
    }
    let mut client = client_for(&remote, &host)?;
    client.connect()?;
    let polled = poll_files(
        &mut client,
        facade,
        &Poll {
            type_id: &type_id,
            remote_id: &remote_id,
            receive_path: &receive_path,
            move_path: &move_path,
            pattern: pattern.as_deref(),
            response: &response,
            charset: &charset,
            limit,
        },
    );
    client.close();
    let file_count = polled?;
    let mut out = BTreeMap::new();
    out.insert("fileCount".to_string(), Value::I64(file_count));
    Ok(out)
}

struct Poll<'a> {
    type_id: &'a str,
    remote_id: &'a str,
    receive_path: &'a str,
    move_path: &'a str,
    /// Java's `String.matches`: the whole name.
    pattern: Option<&'a str>,
    response: &'a str,
    charset: &'a str,
    limit: Option<i64>,
}

fn poll_files(client: &mut SftpClient, facade: &EntityFacade, poll: &Poll<'_>) -> Result<i64, LibError> {
    let (type_id, remote_id) = (poll.type_id, poll.remote_id);
    let mut files = BTreeSet::new();
    for name in client.ls_files(poll.receive_path)? {
        let matched = match poll.pattern {
            Some(pattern) => moqui_lib::object::regex_full_match(&name, pattern)
                .map_err(|err| LibError::new(format!("receiveFilePattern {pattern} of {type_id}: {err}")))?,
            None => true,
        };
        if matched {
            files.insert(name);
        }
    }
    tracing::info!(
        "SFTP Poll type {type_id} remote {remote_id} found matching files ({}): {files:?}",
        files.len()
    );
    if let Some(limit) = poll.limit {
        files = files
            .into_iter()
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .collect();
        tracing::info!(
            "SFTP Poll type {type_id} remote {remote_id} has limit {limit}, getting only files ({}): {files:?}",
            files.len()
        );
    }
    let file_count = i64::try_from(files.len()).unwrap_or(i64::MAX);
    let removes = matches!(poll.response, "MsgRrDelete" | "MsgRrMove");
    for filename in &files {
        if removes {
            let existing = existing_messages(facade, poll, filename)?;
            if !existing.is_empty() {
                tracing::warn!(
                    "SFTP poll file {filename} for type {type_id} remote {remote_id} already in SystemMessage records {existing:?}"
                );
                continue;
            }
        }
        let file_path = format!("{}/{filename}", poll.receive_path);
        let message_text = client.get_text(&file_path, poll.charset)?;
        // A new transaction, so the message is committed before the file is deleted or moved.
        let mut receive = BTreeMap::new();
        receive.insert("systemMessageTypeId".to_string(), Value::string(type_id));
        receive.insert("systemMessageRemoteId".to_string(), Value::string(remote_id));
        receive.insert("remoteMessageId".to_string(), Value::string(filename));
        receive.insert("messageText".to_string(), Value::string(message_text));
        let options = CallOptions {
            disable_authz: true,
            require_new_transaction: Some(true),
            ..CallOptions::default()
        };
        let result = match sync_with(RECEIVE, &receive, &options) {
            Ok(result) => result,
            Err(err) => {
                add_error(err.message().to_string());
                BTreeMap::new()
            }
        };
        if has_error() {
            tracing::error!(
                "SFTP poll error receiving SystemMessage, stopping poll for type {type_id} remote {remote_id}"
            );
            return Ok(file_count);
        }
        let ids = text_of(result.get("systemMessageIdList"));
        tracing::info!("SFTP poll type {type_id} remote {remote_id} saved file {filename} in messages {ids}");
        match poll.response {
            "MsgRrDelete" => {
                client.rm(&file_path)?;
                tracing::info!("SFTP poll SystemMessage {ids} type {type_id} deleted {file_path}");
            }
            "MsgRrMove" if !poll.move_path.is_empty() => {
                let moved = client.move_file(&file_path, poll.move_path, true)?;
                tracing::info!("SFTP poll SystemMessage {ids} type {type_id} moved from {file_path} to {moved}");
            }
            "MsgRrMove" => tracing::error!(
                "SFTP poll SystemMessage {ids} created for remote file {filename} and configured to move but no receiveMovePath set on SystemMessageType {type_id}"
            ),
            _ => {}
        }
    }
    Ok(file_count)
}

/// IDs of the messages already received from this file that are not rejected, cancelled, or in error.
fn existing_messages(facade: &EntityFacade, poll: &Poll<'_>, filename: &str) -> Result<Vec<String>, LibError> {
    let rows = facade
        .find(MESSAGE)?
        .disable_authz()
        .eq("systemMessageRemoteId", Value::string(poll.remote_id))
        .eq("systemMessageTypeId", Value::string(poll.type_id))
        .eq("remoteMessageId", Value::string(filename))
        .list()?;
    Ok(rows
        .iter()
        .filter(|row| {
            !matches!(
                field(row, "statusId").as_str(),
                "SmsgRejected" | "SmsgCancelled" | "SmsgError"
            )
        })
        .map(|row| field(row, "systemMessageId"))
        .collect())
}

/// The client for a remote: its host, username, key (with the public key) or password, and pinned host key.
/// Under the read fence or in a sim preview an SFTP send or poll is refused, and in sim mode denied, before it reads a
/// message or touches a remote file (the client's connect refuses as well).
fn refuse_outside(facade: &EntityFacade, service: &str) -> Result<(), LibError> {
    crate::entity::fence::check_effect(crate::entity::fence::Effect::RemoteCall, service)?;
    if facade.sim_active() {
        return Err(LibError::new(format!("{service} denied because sim mode is active")));
    }
    Ok(())
}

fn client_for(remote: &EntityValue, host: &str) -> Result<SftpClient, LibError> {
    let mut client = SftpClient::new(host, &field(remote, "username"), None)?;
    let private_key = field(remote, "privateKey");
    if private_key.is_empty() {
        client.password(&field(remote, "password"));
    } else {
        let public_key = field(remote, "publicKey");
        client.public_key_pkcs8(&private_key, Some(public_key.as_str()));
    }
    client.host_key_fingerprint(&field(remote, "sshHostKeyFingerprint"));
    Ok(client)
}

/// `ec.resource.expand(type.<name>, null, names, false)`. A Groovy `?:` is refused with the field named, since the
/// expansion is JavaScript; `None` after the error is added.
fn expand_path(message_type: &EntityValue, name: &str, names: &[(&str, Value)]) -> Result<Option<String>, LibError> {
    let template = field(message_type, name);
    if template.is_empty() {
        return Ok(Some(String::new()));
    }
    if template.contains("?:") {
        add_error(format!(
            "SystemMessageType {} {name} uses the Groovy ?: operator; expressions are JavaScript, use ?? instead: {template}",
            field(message_type, "systemMessageTypeId")
        ));
        return Ok(None);
    }
    let _frame = crate::script::ContextFrame::push();
    for (key, value) in names {
        crate::script::set_context_name(key, value.clone())?;
    }
    crate::script::expand(&template)
        .map(Some)
        .map_err(|err| LibError::new(format!("{name} of SystemMessageType: {}", err.message())))
}

fn format_date(value: &Value, pattern: &str) -> Result<String, LibError> {
    crate::l10n::format_value(value, Some(pattern)).map_err(|err| LibError::new(err.message()))
}

fn text_value(text: &str) -> Value {
    if text.is_empty() {
        Value::Null
    } else {
        Value::string(text)
    }
}

fn find_one(facade: &EntityFacade, entity: &str, field: &str, value: &str) -> Result<Option<EntityValue>, LibError> {
    if value.is_empty() {
        return Ok(None);
    }
    facade
        .find(entity)?
        .disable_authz()
        .eq(field, Value::string(value))
        .one()
}

fn field(row: &EntityValue, name: &str) -> String {
    text_of(row.get(name))
}
