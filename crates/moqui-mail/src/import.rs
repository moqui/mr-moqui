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
//!
//! The import is bounded by a window of messages, not by the size of the archive or of the folder
//! (plans/High-Level-Review.md HL-011): each source is read as a stream (an upload from its spool file, a location
//! through `ResourceFacade::open`), an mbox one message at a time, and the messages are taken [`BATCH`] at a time
//! (or [`BATCH_BYTES`]), whose Message-IDs are looked for in the folder in one query. The Message-IDs taken in this
//! import are kept, so a message twice in the archive is imported once; one with no Message-ID is never taken for a
//! duplicate. Each message is stored in a transaction of its own.

use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader, Read};

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value};

use super::implementation::{find_one, force_new, note_error};
use super::store::{self, parse_message, Placement, MESSAGE};
use super::sync::{value_number, FOLDER};
use moqui_core::context::ExecutionContext;
use moqui_core::entity::{Condition, EntityFacade};
use moqui_core::service::util::{field_text, has_error, now_millis, param_text};

const SERVICES: &str = "org.moqui.impl.EmailClientServices";
/// The most errors reported back; the rest are only logged.
const ERRORS_SHOWN: usize = 20;
/// Messages parsed and checked against the folder at a time.
const BATCH: usize = 100;
/// The parsed size past which a batch is taken before it has [`BATCH`] messages.
const BATCH_BYTES: i64 = 16 * 1024 * 1024;

#[cfg(test)]
thread_local! {
    /// A test's batch size, for the de-duplication across batches.
    pub(crate) static TEST_BATCH: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

fn batch_size() -> usize {
    #[cfg(test)]
    if let Some(size) = TEST_BATCH.with(std::cell::Cell::get) {
        return size.max(1);
    }
    BATCH
}

/// A file to import: its name and its content as a stream.
struct Source {
    name: String,
    reader: Box<dyn Read + Send>,
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
                match file.reader() {
                    Ok(reader) => out.push(Source {
                        name: file.file_name.clone(),
                        reader,
                    }),
                    // the others are still imported
                    Err(err) => tracing::error!("the uploaded file {} was not read: {}", file.file_name, err.message()),
                }
            }
        }
        Value::Bytes(bytes) => out.push(Source {
            name: "message.eml".to_string(),
            reader: Box::new(std::io::Cursor::new(bytes.clone())),
        }),
        _ => {}
    }
}

/// The content at `location`, as a stream a window at a time.
fn read_location(location: &str) -> Result<Box<dyn Read + Send>, String> {
    let opened = ExecutionContext::with_current(|ctx| ctx.factory().resource().open(location))
        .ok_or_else(|| "no execution context is entered".to_string())?
        .map_err(|err| err.message().to_string())?
        .ok_or_else(|| "nothing is there".to_string())?;
    if !opened.stat.is_file {
        return Err("it is not a file".to_string());
    }
    Ok(Box::new(moqui_core::resource::source::SourceReader::new(opened.source)))
}

/// The messages of a source, one at a time: an mbox (by name, else by its first line) each without its `From ` line
/// and with `>From ` unquoted, else the one message the source is.
fn messages(source: Source) -> Box<dyn Iterator<Item = std::io::Result<Vec<u8>>>> {
    let name = source.name.to_ascii_lowercase();
    let mut reader = BufReader::with_capacity(64 * 1024, source.reader);
    let mbox = name.ends_with(".mbox")
        || name.ends_with(".mbx")
        || reader.fill_buf().is_ok_and(|head| head.starts_with(b"From "));
    if mbox {
        Box::new(
            mail_parser::mailbox::mbox::MessageIterator::new(reader)
                .map(|message| message.map(mail_parser::mailbox::mbox::Message::unwrap_contents)),
        )
    } else {
        let mut raw = Vec::new();
        Box::new(std::iter::once(reader.read_to_end(&mut raw).map(|_| raw)))
    }
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

/// One import's destination and what it did.
struct Import<'a> {
    facade: &'a EntityFacade,
    account_id: &'a str,
    folder_id: &'a str,
    status: &'static str,
    archived: bool,
    /// The Message-IDs taken in this import, so one twice in the archive is imported once.
    seen: HashSet<String>,
    imported: i64,
    duplicates: i64,
    failed: i64,
    ids: Vec<Value>,
    errors: Vec<String>,
}

impl Import<'_> {
    /// The Message-IDs of `wanted` already in the folder: one query by the account, folder and ids.
    fn in_folder(&self, wanted: Vec<Value>) -> Result<HashSet<String>, LibError> {
        if wanted.is_empty() {
            return Ok(HashSet::new());
        }
        Ok(self
            .facade
            .find(MESSAGE)?
            .eq("emailAccountId", Value::string(self.account_id))
            .eq("emailFolderId", Value::string(self.folder_id))
            .condition(Condition::is_in("messageId", wanted))
            .select_field("messageId")
            .list()?
            .values()
            .iter()
            .map(|row| field_text(row, "messageId"))
            .collect())
    }

    /// Store the messages of `batch` that are not duplicates, each in a transaction of its own, and empty it.
    fn store_batch(&mut self, batch: &mut Vec<(String, store::Parsed)>) {
        if batch.is_empty() {
            return;
        }
        let wanted: Vec<Value> = batch
            .iter()
            .map(|(_, parsed)| parsed.message_id.as_str())
            .filter(|id| !id.is_empty() && !self.seen.contains(*id))
            .collect::<HashSet<_>>()
            .into_iter()
            .map(Value::string)
            .collect();
        let existing = match self.in_folder(wanted) {
            Ok(existing) => existing,
            // without the check a message could be imported twice: these are not imported
            Err(err) => {
                tracing::error!(
                    "Could not check {} messages for email folder {} against those there: {err}",
                    batch.len(),
                    self.folder_id
                );
                for (label, _) in batch.drain(..) {
                    self.errors.push(format!("{label}: {err}"));
                    self.failed += 1;
                }
                return;
            }
        };
        for (label, parsed) in batch.drain(..) {
            if !parsed.message_id.is_empty()
                && (existing.contains(&parsed.message_id) || !self.seen.insert(parsed.message_id.clone()))
            {
                self.duplicates += 1;
                continue;
            }
            let place = Placement {
                account_id: self.account_id,
                folder_id: Some(self.folder_id),
                remote_folder_id: None,
                remote_uid: None,
                remote_uidl: None,
                status_id: self.status,
                read: read_state(&parsed) || self.status != "ES_RECEIVED",
                flagged: false,
                answered: false,
                received_millis: parsed.sent_millis.or(Some(now_millis())),
                server_size: None,
                archived: self.archived,
            };
            let facade = self.facade;
            match force_new(facade, || store::store_message(facade, &parsed, &place)) {
                Ok(id) => {
                    self.imported += 1;
                    self.ids.push(Value::string(id));
                }
                Err(err) => {
                    tracing::error!("Could not import {label} into email folder {}: {err}", self.folder_id);
                    self.errors.push(format!("{label}: {err}"));
                    self.failed += 1;
                }
            }
        }
    }
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
            Ok(reader) => sources.push(Source {
                name: location.rsplit('/').next().unwrap_or_default().to_string(),
                reader,
            }),
            Err(text) => errors.push(format!("{location}: {text}")),
        }
    }
    let text = param_text(parameters, "messageText");
    if !text.is_empty() {
        sources.push(Source {
            name: "message.eml".to_string(),
            reader: Box::new(std::io::Cursor::new(text.into_bytes())),
        });
    }
    if sources.is_empty() && errors.is_empty() {
        note_error("Choose .eml or .mbox files to import");
        return Ok(out);
    }
    let kind = field_text(&folder, "folderKindEnumId");
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut import = Import {
        facade,
        account_id: &account_id,
        folder_id: &folder_id,
        status: match kind.as_str() {
            "EfkSent" => "ES_SENT",
            "EfkDrafts" => "ES_DRAFT",
            _ => "ES_RECEIVED",
        },
        archived: kind == "EfkArchive",
        seen: HashSet::new(),
        imported: 0,
        duplicates: 0,
        failed: 0,
        ids: Vec::new(),
        errors,
    };
    let size = batch_size();
    let mut batch: Vec<(String, store::Parsed)> = Vec::new();
    let mut batch_bytes = 0_i64;
    for source in sources {
        let name = source.name.clone();
        for (index, raw) in messages(source).enumerate() {
            let label = format!("{name} message {}", index + 1);
            let raw = match raw {
                Ok(raw) => raw,
                // the reader is broken: the rest of this source cannot be read, the other sources go on
                Err(err) => {
                    tracing::error!("Could not read {label} for email folder {folder_id}: {err}");
                    import.errors.push(format!("{label}: {err}"));
                    import.failed += 1;
                    break;
                }
            };
            match parse_message(&raw) {
                Ok(parsed) => {
                    batch_bytes += parsed.size;
                    batch.push((label, parsed));
                }
                Err(text) => {
                    tracing::warn!("Not importing {label} into email folder {folder_id}: {text}");
                    import.errors.push(format!("{label}: {text}"));
                    import.failed += 1;
                }
            }
            drop(raw);
            if batch.len() >= size || batch_bytes >= BATCH_BYTES {
                import.store_batch(&mut batch);
                batch_bytes = 0;
            }
        }
    }
    import.store_batch(&mut batch);
    let Import {
        imported,
        duplicates,
        failed,
        ids,
        mut errors,
        ..
    } = import;
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
