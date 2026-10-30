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

//! `import#EmailMessages`: `.eml` files (one message each) and `.mbox` files (many) into a folder of a local or synced
//! account. The messages are kept here only: nothing is appended to the server. One already in the folder (same
//! Message-ID) is not imported again; one that does not parse or cannot be stored is logged and the rest go on. The
//! read state comes from the mbox's own headers (`Status`, `X-Mozilla-Status`, Gmail's `X-Gmail-Labels`), read when
//! they say nothing.

use std::collections::{BTreeMap, HashSet};

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value};

use super::implementation::{find_one, force_new, note_error};
use super::store::{self, parse_message, Placement, MESSAGE};
use super::sync::{value_number, FOLDER};
use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;
use moqui_core::service::util::{field_text, has_error, now_millis, param_text};

const SERVICES: &str = "org.moqui.impl.EmailClientServices";
/// The most errors reported back; the rest are only logged.
const ERRORS_SHOWN: usize = 20;

/// A file to import: its name and bytes.
struct Source {
    name: String,
    bytes: Vec<u8>,
}

fn uploaded(value: &Value, out: &mut Vec<Source>) {
    match value {
        Value::List(items) => {
            for item in items {
                uploaded(item, out);
            }
        }
        Value::Object(object) => {
            if let Some(file) = object.downcast::<moqui_core::web::multipart::UploadedFile>() {
                match file.read_bytes() {
                    Ok(bytes) => out.push(Source {
                        name: file.file_name.clone(),
                        bytes,
                    }),
                    // the others are still imported
                    Err(err) => tracing::error!("the uploaded file {} was not read: {}", file.file_name, err.message()),
                }
            }
        }
        Value::Bytes(bytes) => out.push(Source {
            name: "message.eml".to_string(),
            bytes: bytes.clone(),
        }),
        _ => {}
    }
}

fn read_location(location: &str) -> Result<Vec<u8>, String> {
    let found = ExecutionContext::with_current(|ctx| ctx.factory().resource().get(location))
        .ok_or_else(|| "no execution context is entered".to_string())?;
    let reference = found.map_err(|err| err.message().to_string())?;
    if !reference.exists() {
        return Err("nothing is there".to_string());
    }
    reference.read_bytes().map_err(|err| err.message().to_string())
}

/// An mbox: by name, else by its first line.
fn is_mbox(source: &Source) -> bool {
    let name = source.name.to_ascii_lowercase();
    name.ends_with(".mbox") || name.ends_with(".mbx") || source.bytes.starts_with(b"From ")
}

/// The messages of an mbox, each without its `From ` line and with `>From ` unquoted.
fn split_mbox(bytes: &[u8]) -> Vec<Vec<u8>> {
    mail_parser::mailbox::mbox::MessageIterator::new(bytes)
        .filter_map(Result::ok)
        .map(mail_parser::mailbox::mbox::Message::unwrap_contents)
        .collect()
}

/// Whether an imported message was read, by the headers mail programs write in an mbox.
fn read_state(parsed: &store::Parsed) -> bool {
    let gmail = parsed.header("X-Gmail-Labels");
    if !gmail.is_empty() {
        return !gmail
            .split(',')
            .any(|label| label.trim().eq_ignore_ascii_case("Unread"));
    }
    let mozilla = parsed.header("X-Mozilla-Status");
    if let Ok(bits) = u32::from_str_radix(mozilla.trim(), 16) {
        return bits & 0x0001 != 0;
    }
    let status = parsed.header("Status");
    if !status.is_empty() {
        return status.contains('R');
    }
    true
}

/// `import#EmailMessages`.
pub(super) fn import_messages(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let folder_id = param_text(parameters, "emailFolderId");
    let Some(folder) = find_one(facade, FOLDER, "emailFolderId", &folder_id)? else {
        note_error(format!("Email folder {folder_id} not found"));
        return Ok(out);
    };
    let account_id = field_text(&folder, "emailAccountId");
    let mut access_in = BTreeMap::new();
    access_in.insert("emailAccountId".to_string(), Value::string(&account_id));
    let access = moqui_core::service::sync(&format!("{SERVICES}.get#EmailAccountAccess"), &access_in)
        .map_err(|err| LibError::new(err.message()))?;
    if has_error() {
        return Ok(out);
    }
    let authz_off = ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false);
    let level = value_number(access.get("level")).unwrap_or(0);
    if !authz_off && level < 1 {
        note_error(format!("Email folder {folder_id} not found"));
        return Ok(out);
    }
    if !authz_off && level < 2 {
        note_error(format!("You may not change the mail of email account {account_id}"));
        return Ok(out);
    }
    let mut sources = Vec::new();
    for name in ["file", "fileList"] {
        if let Some(value) = parameters.get(name) {
            uploaded(value, &mut sources);
        }
    }
    let mut errors: Vec<String> = Vec::new();
    let mut locations: Vec<String> = Vec::new();
    for name in ["location", "locationList"] {
        match parameters.get(name) {
            Some(Value::List(items)) => locations.extend(items.iter().map(to_plain_string)),
            Some(value) if !value.is_null() => locations.push(to_plain_string(value)),
            _ => {}
        }
    }
    for location in locations.into_iter().filter(|location| !location.is_empty()) {
        match read_location(&location) {
            Ok(bytes) => sources.push(Source {
                name: location.rsplit('/').next().unwrap_or_default().to_string(),
                bytes,
            }),
            Err(text) => errors.push(format!("{location}: {text}")),
        }
    }
    let text = param_text(parameters, "messageText");
    if !text.is_empty() {
        sources.push(Source {
            name: "message.eml".to_string(),
            bytes: text.into_bytes(),
        });
    }
    if sources.is_empty() && errors.is_empty() {
        note_error("Choose .eml or .mbox files to import");
        return Ok(out);
    }
    let kind = field_text(&folder, "folderKindEnumId");
    let status = match kind.as_str() {
        "EfkSent" => "ES_SENT",
        "EfkDrafts" => "ES_DRAFT",
        _ => "ES_RECEIVED",
    };
    let mut known: HashSet<String> = facade
        .find(MESSAGE)?
        .eq("emailAccountId", Value::string(&account_id))
        .eq("emailFolderId", Value::string(&folder_id))
        .select_field("messageId")
        .list()?
        .values()
        .iter()
        .map(|row| field_text(row, "messageId"))
        .filter(|id| !id.is_empty())
        .collect();
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let (mut imported, mut duplicates, mut failed) = (0i64, 0i64, 0i64);
    let mut ids = Vec::new();
    for source in sources {
        let messages = if is_mbox(&source) {
            split_mbox(&source.bytes)
        } else {
            vec![source.bytes]
        };
        for (index, raw) in messages.into_iter().enumerate() {
            let label = format!("{} message {}", source.name, index + 1);
            let parsed = match parse_message(&raw) {
                Ok(parsed) => parsed,
                Err(text) => {
                    tracing::warn!("Not importing {label} into email folder {folder_id}: {text}");
                    errors.push(format!("{label}: {text}"));
                    failed += 1;
                    continue;
                }
            };
            if !parsed.message_id.is_empty() && !known.insert(parsed.message_id.clone()) {
                duplicates += 1;
                continue;
            }
            let place = Placement {
                account_id: &account_id,
                folder_id: Some(&folder_id),
                remote_folder_id: None,
                remote_uid: None,
                remote_uidl: None,
                status_id: status,
                read: read_state(&parsed) || status != "ES_RECEIVED",
                flagged: false,
                answered: false,
                received_millis: parsed.sent_millis.or(Some(now_millis())),
                server_size: None,
                archived: kind == "EfkArchive",
            };
            match force_new(facade, || store::store_message(facade, &parsed, &place)) {
                Ok(id) => {
                    imported += 1;
                    ids.push(Value::string(id));
                }
                Err(err) => {
                    tracing::error!("Could not import {label} into email folder {folder_id}: {err}");
                    errors.push(format!("{label}: {err}"));
                    failed += 1;
                }
            }
        }
    }
    if imported > 0 {
        let mut counts = BTreeMap::new();
        counts.insert(
            "emailAccountIdList".to_string(),
            Value::List(vec![Value::string(&account_id)]),
        );
        let _ = moqui_core::service::sync(&format!("{SERVICES}.publish#AccountCounts"), &counts);
    }
    errors.truncate(ERRORS_SHOWN);
    out.insert("importedCount".into(), Value::I64(imported));
    out.insert("duplicateCount".into(), Value::I64(duplicates));
    out.insert("failedCount".into(), Value::I64(failed));
    out.insert("emailMessageIdList".into(), Value::List(ids));
    out.insert(
        "errorList".into(),
        Value::List(errors.into_iter().map(Value::String).collect()),
    );
    Ok(out)
}
