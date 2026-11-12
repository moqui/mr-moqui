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

use super::{add_error, has_error, integer_of, millis_of, now_millis, param_text, stamp, take_errors, text_of};
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
/// order up to `fileCountLimit`, then delete or move it as `receiveResponseEnumId` says. Each file is on its own
/// (runtime/component/mr-mantle/plan/payments-shipping-edi.md section 2): a file that cannot be read, received or
/// removed is logged by name, stays on the server and is in `failedFileList`, and the poll goes on with the others. A
/// file is deleted or moved only after its receipt is durable (the receive service committed a message, or answered
/// that one already holds it); a file whose text a message already holds was received before its cleanup failed, so it
/// is cleaned up now and not received again.
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
    let polled = polled?;
    let mut out = BTreeMap::new();
    out.insert("fileCount".to_string(), Value::I64(polled.file_count));
    out.insert(
        "failedFileList".to_string(),
        Value::List(polled.failed.into_iter().map(Value::String).collect()),
    );
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

/// What a poll did: the files it took up and those it left on the server after a failure.
struct Polled {
    file_count: i64,
    failed: Vec<String>,
}

/// What became of one file.
#[derive(Debug, PartialEq, Eq)]
enum FileOutcome {
    /// Received now, in the messages named.
    Received(String),
    /// A message already held its text: received before, cleaned up now when the type removes files.
    AlreadyReceived(Vec<String>),
}

/// The remote file operations a poll uses, so the per-file handling is tested without a server.
trait RemoteFiles {
    fn ls_files(&mut self, path: &str) -> Result<Vec<String>, LibError>;
    fn get_text(&mut self, path: &str, charset: &str) -> Result<String, LibError>;
    fn rm(&mut self, path: &str) -> Result<(), LibError>;
    fn move_file(&mut self, file_path: &str, dest_dir: &str) -> Result<String, LibError>;
}

impl RemoteFiles for SftpClient {
    fn ls_files(&mut self, path: &str) -> Result<Vec<String>, LibError> {
        SftpClient::ls_files(self, path)
    }
    fn get_text(&mut self, path: &str, charset: &str) -> Result<String, LibError> {
        SftpClient::get_text(self, path, charset)
    }
    fn rm(&mut self, path: &str) -> Result<(), LibError> {
        SftpClient::rm(self, path)
    }
    fn move_file(&mut self, file_path: &str, dest_dir: &str) -> Result<String, LibError> {
        SftpClient::move_file(self, file_path, dest_dir, true)
    }
}

/// The local side of a poll: the messages that already hold a file, and the receive of a new one.
trait Receipts {
    /// The IDs of the messages (not rejected, cancelled or in error) of this file whose text is `text`.
    fn holding(&mut self, filename: &str, text: &str) -> Result<Vec<String>, LibError>;
    /// Receive the file in a transaction of its own: the IDs of the messages kept, or of the one that already holds
    /// it (a duplicate); empty when nothing was kept. An error is the receive's failure.
    fn receive(&mut self, filename: &str, text: String) -> Result<Vec<String>, LibError>;
}

struct ServiceReceipts<'a> {
    facade: &'a EntityFacade,
    type_id: &'a str,
    remote_id: &'a str,
}

impl Receipts for ServiceReceipts<'_> {
    fn holding(&mut self, filename: &str, text: &str) -> Result<Vec<String>, LibError> {
        existing_messages(self.facade, self.type_id, self.remote_id, filename, text)
    }

    fn receive(&mut self, filename: &str, text: String) -> Result<Vec<String>, LibError> {
        let mut receive = BTreeMap::new();
        receive.insert("systemMessageTypeId".to_string(), Value::string(self.type_id));
        receive.insert("systemMessageRemoteId".to_string(), Value::string(self.remote_id));
        receive.insert("remoteMessageId".to_string(), Value::string(filename));
        receive.insert("messageText".to_string(), Value::string(text));
        // A new transaction, so the message is committed before the file is deleted or moved.
        let options = CallOptions {
            disable_authz: true,
            require_new_transaction: Some(true),
            ..CallOptions::default()
        };
        let result = sync_with(RECEIVE, &receive, &options);
        // Each file's errors are its own: taken here so the next file's receive starts clean.
        let errors = take_errors();
        let result = result?;
        if has_error() || !errors.is_empty() {
            return Err(LibError::new(errors));
        }
        let mut ids: Vec<String> = match result.get("systemMessageIdList") {
            Some(Value::List(items)) => items.iter().map(|item| text_of(Some(item))).collect(),
            other => vec![text_of(other)],
        };
        ids.retain(|id| !id.is_empty());
        let duplicate = text_of(result.get("duplicateOfId"));
        if ids.is_empty() && !duplicate.is_empty() {
            ids.push(duplicate);
        }
        Ok(ids)
    }
}

fn poll_files(client: &mut SftpClient, facade: &EntityFacade, poll: &Poll<'_>) -> Result<Polled, LibError> {
    let mut receipts = ServiceReceipts {
        facade,
        type_id: poll.type_id,
        remote_id: poll.remote_id,
    };
    poll_each(client, &mut receipts, poll)
}

fn poll_each(files_at: &mut dyn RemoteFiles, receipts: &mut dyn Receipts, poll: &Poll<'_>) -> Result<Polled, LibError> {
    let (type_id, remote_id) = (poll.type_id, poll.remote_id);
    let mut files = BTreeSet::new();
    // The listing and the pattern are the connection's: when they fail no file can be trusted, so the poll stops.
    for name in files_at.ls_files(poll.receive_path)? {
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
    let mut failed = Vec::new();
    for filename in &files {
        match poll_file(files_at, receipts, poll, filename) {
            Ok(FileOutcome::Received(ids)) => {
                tracing::info!("SFTP poll type {type_id} remote {remote_id} saved file {filename} in messages {ids}");
            }
            Ok(FileOutcome::AlreadyReceived(ids)) => {
                tracing::info!(
                    "SFTP poll file {filename} for type {type_id} remote {remote_id} already in SystemMessage records {ids:?}"
                );
            }
            Err(err) => {
                tracing::error!(
                    "SFTP poll type {type_id} remote {remote_id} file {filename} failed and stays on the server: {}",
                    err.message()
                );
                failed.push(filename.clone());
            }
        }
    }
    Ok(Polled { file_count, failed })
}

/// Receive one file, then remove it once its receipt is durable.
fn poll_file(
    files_at: &mut dyn RemoteFiles,
    receipts: &mut dyn Receipts,
    poll: &Poll<'_>,
    filename: &str,
) -> Result<FileOutcome, LibError> {
    let file_path = format!("{}/{filename}", poll.receive_path);
    let message_text = files_at.get_text(&file_path, poll.charset)?;
    let held = receipts.holding(filename, &message_text)?;
    let outcome = if held.is_empty() {
        let ids = receipts.receive(filename, message_text)?;
        if ids.is_empty() {
            // Nothing durable holds the file, so it stays for the next poll.
            return Err(LibError::new("the receive service kept no message"));
        }
        FileOutcome::Received(ids.join(","))
    } else {
        FileOutcome::AlreadyReceived(held)
    };
    match poll.response {
        "MsgRrDelete" => {
            files_at.rm(&file_path)?;
            tracing::info!("SFTP poll type {} deleted {file_path}", poll.type_id);
        }
        "MsgRrMove" if !poll.move_path.is_empty() => {
            let moved = files_at.move_file(&file_path, poll.move_path)?;
            tracing::info!("SFTP poll type {} moved {file_path} to {moved}", poll.type_id);
        }
        "MsgRrMove" => tracing::error!(
            "SFTP poll file {filename} received and configured to move but no receiveMovePath set on SystemMessageType {}",
            poll.type_id
        ),
        _ => {}
    }
    Ok(outcome)
}

/// IDs of the messages already received from this file with this text that are not rejected, cancelled, or in error.
/// A file of the same name with other text is a new file, received again. The text is compared in the query, and only
/// the newest few IDs and statuses are read, so a name reused for many files does not load their texts each poll.
fn existing_messages(
    facade: &EntityFacade,
    type_id: &str,
    remote_id: &str,
    filename: &str,
    text: &str,
) -> Result<Vec<String>, LibError> {
    let rows = facade
        .find(MESSAGE)?
        .disable_authz()
        .eq("systemMessageRemoteId", Value::string(remote_id))
        .eq("systemMessageTypeId", Value::string(type_id))
        .eq("remoteMessageId", Value::string(filename))
        .eq("messageText", Value::string(text))
        .select_field("systemMessageId")
        .select_field("statusId")
        .order_by("initDate", true)
        .limit(20)
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
    SftpClient::for_remote(remote, host, &field(remote, "username"))
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

#[cfg(test)]
#[path = "sftp_tests.rs"]
mod tests;
