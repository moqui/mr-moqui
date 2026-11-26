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

//! The parts of `ec.resource` that drives and file browsers use (`org.moqui.resource.FileServices`): change sets, version
//! notes, last changes and change logs, the bytes under a folder, image thumbnails, PDF and spreadsheet previews, the
//! text of a file for search, zip archives, and signed grants. Guide: `docs/data/Revision-Management.md`.
//!
//! ```text
//! ec.resource.makeChangeSet(root)          -> builder: write(path, content), delete(path), move(from, to), mkdir(path),
//!                                             author(userId, date): the versions are by that user at that date,
//!                                             commit(message) -> {versionName, applied, failures: [{path, error}]}
//! ref.putText(text, note), ref.putBytes(bytes, note), ref.putStream(stream, note): a version with a note
//! ec.resource.putContent(location, content, note)  content: text, Uint8Array, an uploaded file or a stream
//! ec.resource.getLastChange(location)      -> {isFile, size, versionName, date, userId, note} or null
//! ec.resource.getChildChanges(location)    -> [{name, isFile, size, versionName, date, userId, note}]
//! ec.resource.getChangeLog(location, limit)-> [{versionName, previousVersionName, versionDate, userId, note, paths}]
//! ec.resource.getTreeSize(location)        -> bytes
//! ec.resource.getContentAddress(location)  -> {kind, id, size, tag} or null: what a pinned revision is recorded by
//! ec.resource.makeThumbnail(source, target, maxSize) -> {width, height, contentType} or null (not an image)
//! ec.resource.makeStickerThumbnail(source, target, maxSide, thumbSize)
//!                                          -> {format, contentType, width, height, thumbWidth, thumbHeight} or
//!                                             {reasonCode: badType | tooLarge, format, width, height}: a PNG, GIF or WebP
//!                                             (its type read from its bytes), no side over maxSide (read from its header,
//!                                             before it is decoded), decoded within a small memory limit
//! ec.resource.makePdfPreview(source, target, maxSize) -> {width, height, pages, contentType} or null (no PDF engine,
//!                                             or the PDF does not open: logged)
//! ec.resource.readSheetPreview(location, sheetName, maxRows, maxCols)
//!                                          -> {sheetNames, sheetName, rows, truncated} or null (not a spreadsheet)
//! ec.resource.extractText(location, cacheKey, maxChars, cacheOnly) -> the text of a PDF, spreadsheet or text file,
//!                                             kept in runtime://tmp/file-text/<cacheKey>.txt; "" when it has none;
//!                                             null with cacheOnly when it was not extracted yet
//! ec.resource.writeZip(target, [{path, location}]) -> {count, size, failures: [{path, error}]}: built in a file, each
//!                                             file read as a stream, never whole in memory
//! ec.resource.makeGrant(key, subject, ttlSeconds)  -> token;  ec.resource.checkGrant(key, subject, token) -> boolean
//! ```

#[cfg(feature = "image")]
use std::io::Cursor;

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use moqui_lib::object::Value;

use super::{method, with_ec, Call, HostError, Method, Ret};
use crate::context::{ResourceChange, VersionAuthor};

/// `(function (ec, host) { ... })`: the `ec.resource` functions over the hidden functions.
macro_rules! files_js {
    () => {
        r#"(function (ec, host) {
    // the bytes of a change: text, a Uint8Array or ArrayBuffer, an uploaded file (it stays the file), or a stream
    const content = (value) => {
        if (value == null) return "";
        if (typeof value === "string") return value;
        if (typeof value.readAllBytes === "function") return value.readAllBytes();
        return value;
    };
    ec.resource.makeChangeSet = function (root) {
        if (root == null || String(root) === "") throw new Error("makeChangeSet needs a root location");
        const changes = [];
        let author = null;
        const builder = {
            root: String(root),
            write(path, value) { changes.push({ op: "write", path: String(path), content: content(value) }); return builder; },
            delete(path) { changes.push({ op: "delete", path: String(path) }); return builder; },
            move(from, to) { changes.push({ op: "move", from: String(from), to: String(to) }); return builder; },
            mkdir(path) { changes.push({ op: "mkdir", path: String(path) }); return builder; },
            // the versions this change set writes are by userId at date (a timestamp or milliseconds), not the current
            // user now: for a version copied with its history; on git the committer stays the current user
            author(userId, date) {
                author = { userId: userId == null || userId === "" ? null : String(userId),
                    date: date == null || date === "" ? null : date };
                return builder;
            },
            size() { return changes.length; },
            commit(message) {
                return host.filesApplyChangeSet(builder.root, changes, message == null ? null : String(message), author);
            }
        };
        return builder;
    };
    ec.resource.putContent = (location, value, note) =>
        host.filesPutContent(String(location), content(value), note == null ? null : String(note));
    ec.resource.getLastChange = (location) => host.filesLastChange(String(location));
    ec.resource.getChildChanges = (location) => host.filesChildChanges(String(location));
    ec.resource.getChangeLog = (location, limit) => host.filesChangeLog(String(location), limit == null ? 100 : Number(limit));
    ec.resource.getTreeSize = (location) => host.filesTreeSize(String(location));
    ec.resource.getContentAddress = (location) => host.filesContentAddress(String(location));
    ec.resource.imageAvailable = host.filesImageAvailable();
    Object.defineProperty(ec.service, "ssoAvailable", {get: () => host.filesSsoAvailable()});
    ec.resource.makeThumbnail = (source, target, maxSize) =>
        host.filesThumbnail(String(source), String(target), maxSize == null ? 256 : Number(maxSize));
    ec.resource.makeStickerThumbnail = (source, target, maxSide, thumbSize) =>
        host.filesStickerThumbnail(String(source), String(target), maxSide == null ? 512 : Number(maxSide), thumbSize == null ? 128 : Number(thumbSize));
    ec.resource.makePdfPreview = (source, target, maxSize) =>
        host.filesPdfPreview(String(source), String(target), maxSize == null ? 1024 : Number(maxSize));
    ec.resource.readSheetPreview = (location, sheetName, maxRows, maxCols) =>
        host.filesSheetPreview(String(location), sheetName == null ? "" : String(sheetName),
            maxRows == null ? 100 : Number(maxRows), maxCols == null ? 26 : Number(maxCols));
    ec.resource.extractText = (location, cacheKey, maxChars, cacheOnly) =>
        host.filesExtractText(String(location), String(cacheKey), maxChars == null ? 100000 : Number(maxChars), cacheOnly === true);
    ec.resource.writeZip = (target, entries) => host.filesWriteZip(String(target), Array.from(entries || []));
    ec.resource.restoreVersion = (location, versionName, note) =>
        host.filesRestoreVersion(String(location), String(versionName), note == null ? null : String(note));
    ec.resource.getVersionBytes = (location, versionName) => host.filesVersionBytes(String(location), String(versionName));
    ec.resource.contentTypeOf = (filename) => host.filesContentType(filename == null ? "" : String(filename));
    // Java's name for it
    ec.resource.getContentType = ec.resource.contentTypeOf;
    ec.resource.makeGrant = (key, subject, ttlSeconds) =>
        host.filesMakeGrant(String(key), String(subject), ttlSeconds == null ? 3600 : Number(ttlSeconds));
    ec.resource.checkGrant = (key, subject, token) =>
        token == null ? false : host.filesCheckGrant(String(key), String(subject), String(token));
})"#
    };
}
pub(super) use files_js;

/// The files functions of the tools shim.
pub(super) static METHODS: &[Method] = &[
    method!(
        "tools",
        "filesApplyChangeSet",
        [Text, Value, OptValue, OptValue],
        files_apply_change_set
    ),
    method!("tools", "filesPutContent", [Text, Value, OptValue], files_put_content),
    method!("tools", "filesLastChange", [Text], files_last_change),
    method!("tools", "filesChildChanges", [Text], files_child_changes),
    method!("tools", "filesChangeLog", [Text, Number], files_change_log),
    method!("tools", "filesTreeSize", [Text], files_tree_size),
    method!("tools", "filesContentAddress", [Text], files_content_address),
    method!("tools", "filesImageAvailable", [], None, leaf, files_image_available),
    method!("tools", "filesSsoAvailable", [], None, leaf, files_sso_available),
    method!("tools", "filesThumbnail", [Text, Text, Number], files_thumbnail),
    method!(
        "tools",
        "filesStickerThumbnail",
        [Text, Text, Number, Number],
        files_sticker_thumbnail
    ),
    method!("tools", "filesPdfPreview", [Text, Text, Number], files_pdf_preview),
    method!(
        "tools",
        "filesSheetPreview",
        [Text, Text, Number, Number],
        files_sheet_preview
    ),
    method!(
        "tools",
        "filesExtractText",
        [Text, Text, Number, Bool],
        files_extract_text
    ),
    method!("tools", "filesWriteZip", [Text, Value], files_write_zip),
    method!(
        "tools",
        "filesRestoreVersion",
        [Text, Text, OptValue],
        files_restore_version
    ),
    method!("tools", "filesVersionBytes", [Text, Text], files_version_bytes),
    method!("tools", "filesContentType", [Text], None, leaf, files_content_type),
    method!(
        "tools",
        "filesMakeGrant",
        [Text, Text, Number],
        None,
        leaf,
        files_make_grant
    ),
    method!(
        "tools",
        "filesCheckGrant",
        [Text, Text, Text],
        None,
        leaf,
        files_check_grant
    ),
];

fn files_image_available(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(cfg!(feature = "image").into())
}

fn files_sso_available(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::sso::client().is_ok().into())
}

fn with_resources<T>(
    body: impl FnOnce(&crate::context::ResourceFacade) -> Result<T, crate::context::Error>,
) -> Result<T, HostError> {
    with_ec(|ec| body(ec.factory().resource()))?.map_err(HostError::Context)
}

/// The bytes of a change's content as the script passed it: text, binary, an uploaded file or a stream. Anything else
/// (an object or a list, such as a stream that lost its bytes on the way) is refused, never written as its text.
fn content_bytes(value: Value) -> Result<Vec<u8>, HostError> {
    let refused = || HostError::message("a file's content is text, bytes, an uploaded file or a stream");
    match value {
        Value::Null => Ok(Vec::new()),
        Value::Bytes(bytes) => Ok(bytes),
        Value::String(text) => Ok(text.into_bytes()),
        Value::Object(object) => {
            if let Some(file) = object.downcast::<crate::web::multipart::UploadedFile>() {
                file.read_bytes().map_err(|err| HostError::message(err.message()))
            } else if let Some(stream) = object.downcast::<super::StreamBytes>() {
                stream.bytes().map_err(|err| HostError::message(err.message()))
            } else {
                Err(refused())
            }
        }
        Value::Map(_) | Value::List(_) => Err(refused()),
        other => Ok(moqui_lib::object::to_plain_string(&other).into_bytes()),
    }
}

/// The text of a field of a change or an entry, as the script gave it; empty for none.
fn text_field(item: &moqui_lib::object::ValueMap, name: &str) -> String {
    match item.get(name) {
        None | Some(Value::Null) => String::new(),
        Some(value) => moqui_lib::object::to_plain_string(value),
    }
}

/// The items of a list argument that are objects.
fn entries_of(value: Value) -> Vec<moqui_lib::object::ValueMap> {
    match value {
        Value::List(items) => items
            .into_iter()
            .filter_map(|item| match item {
                Value::Map(map) => Some(map),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A note or message argument: its text, none for null or not passed.
fn note_of(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(value) => Some(moqui_lib::object::to_plain_string(value)),
    }
}

/// The `author(userId, date)` of a change set builder as a version author: the date a timestamp or milliseconds.
fn version_author_of(value: Option<&Value>) -> Result<Option<VersionAuthor>, HostError> {
    let Some(Value::Map(map)) = value else {
        return Ok(None);
    };
    let user_id = map
        .get("userId")
        .filter(|value| !matches!(value, Value::Null))
        .map(moqui_lib::object::to_plain_string)
        .filter(|user_id| !user_id.is_empty());
    let date = match map.get("date") {
        None | Some(Value::Null) => None,
        Some(Value::I64(millis)) => Some(*millis),
        #[allow(clippy::cast_possible_truncation)]
        Some(Value::F64(millis)) if millis.is_finite() => Some(*millis as i64),
        Some(other) => match moqui_lib::object::basic_convert(other, "Timestamp").value {
            Value::Timestamp(stamp) => Some(stamp.as_millisecond()),
            _ => {
                return Err(HostError::message(
                    "a change set author's date is a timestamp or milliseconds",
                ))
            }
        },
    };
    Ok((user_id.is_some() || date.is_some()).then_some(VersionAuthor { user_id, date }))
}

/// `commit(message)` of a change set builder: `{versionName, applied, failures: [{path, error}]}`.
fn files_apply_change_set(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let root = call.text_or_empty(0);
    let list = entries_of(call.value_or_null(1));
    let mut changes = Vec::with_capacity(list.len());
    for mut item in list {
        let op = text_field(&item, "op");
        changes.push(match op.as_str() {
            "write" => ResourceChange::Write {
                path: text_field(&item, "path"),
                bytes: content_bytes(item.swap_remove("content").unwrap_or(Value::Null))?,
            },
            "delete" => ResourceChange::Delete {
                path: text_field(&item, "path"),
            },
            "mkdir" => ResourceChange::MakeDirectory {
                path: text_field(&item, "path"),
            },
            "move" => ResourceChange::Move {
                from: text_field(&item, "from"),
                to: text_field(&item, "to"),
            },
            other => return Err(HostError::message(format!("unknown change {other}"))),
        });
    }
    let message = note_of(call.value(2));
    let author = version_author_of(call.value(3))?;
    let outcome = with_resources(|resources| {
        crate::context::with_version_author(author, || {
            resources.apply_change_set(&root, &changes, message.as_deref())
        })
    })?;
    let mut map = indexmap::IndexMap::default();
    map.insert(
        "versionName".to_string(),
        outcome.version_name.map_or(Value::Null, Value::string),
    );
    map.insert(
        "applied".to_string(),
        Value::List(outcome.applied.into_iter().map(Value::string).collect()),
    );
    map.insert(
        "failures".to_string(),
        Value::List(
            outcome
                .failures
                .into_iter()
                .map(|(path, error)| {
                    let mut failure = indexmap::IndexMap::default();
                    failure.insert("path".to_string(), Value::string(path));
                    failure.insert("error".to_string(), Value::string(error));
                    Value::Map(failure)
                })
                .collect(),
        ),
    );
    Ok(Ret::Value(Value::Map(map)))
}

/// `putContent(location, content, note)` and the `put*` methods of a reference given a note.
fn files_put_content(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let bytes = content_bytes(call.value_or_null(1))?;
    let note = note_of(call.value(2));
    with_resources(|resources| resources.write_bytes_noted(&location, &bytes, note.as_deref()))?;
    Ok(Ret::Undefined)
}

fn files_last_change(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let change = with_resources(|resources| resources.last_change(&location))?;
    Ok(Ret::Value(change.map_or(Value::Null, |change| change.to_value())))
}

fn files_child_changes(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let changes = with_resources(|resources| resources.child_changes(&location))?;
    let list = changes
        .into_iter()
        .map(|(name, change)| {
            let Value::Map(mut map) = change.to_value() else {
                return Value::Null;
            };
            map.insert_before(0, "name".to_string(), Value::string(name));
            Value::Map(map)
        })
        .collect();
    Ok(Ret::Value(Value::List(list)))
}

fn files_change_log(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let limit = call.number(1).unwrap_or_default();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let limit = if limit.is_finite() && limit > 0.0 {
        limit as usize
    } else {
        100
    };
    let entries = with_resources(|resources| resources.change_log(&location, limit))?;
    let list = entries
        .into_iter()
        .map(|entry| {
            let version = entry.version;
            let mut map = indexmap::IndexMap::default();
            map.insert("versionName".to_string(), Value::string(version.version_name));
            map.insert(
                "previousVersionName".to_string(),
                version.previous_version_name.map_or(Value::Null, Value::string),
            );
            map.insert("versionDate".to_string(), version.version_date);
            map.insert("userId".to_string(), version.user_id.map_or(Value::Null, Value::string));
            map.insert("note".to_string(), version.note.map_or(Value::Null, Value::string));
            map.insert(
                "paths".to_string(),
                Value::List(entry.paths.into_iter().map(Value::string).collect()),
            );
            Value::Map(map)
        })
        .collect();
    Ok(Ret::Value(Value::List(list)))
}

/// `restoreVersion(location, versionName, note)`: that version written back as the newest; the new version's name.
fn files_restore_version(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let version_name = call.text_or_empty(1);
    let note = note_of(call.value(2));
    let made = with_resources(|resources| resources.restore_version(&location, &version_name, note.as_deref()))?;
    Ok(Ret::Value(made.map_or(Value::Null, Value::string)))
}

/// `getVersionBytes(location, versionName)`: the bytes of a version as a `Uint8Array`, or null.
fn files_version_bytes(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let version_name = call.text_or_empty(1);
    let bytes = with_resources(|resources| resources.read_version(&location, &version_name))?;
    Ok(Ret::Value(bytes.map_or(Value::Null, Value::Bytes)))
}

/// `contentTypeOf(filename)`: the content type of a file name's extension, or null.
fn files_content_type(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let filename = call.text_or_empty(0);
    Ok(Ret::Value(
        moqui_lib::resource::content_type_for_name(&filename).map_or(Value::Null, Value::string),
    ))
}

fn files_tree_size(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let size = with_resources(|resources| resources.tree_size(&location))?;
    Ok(Ret::Value(Value::I64(i64::try_from(size).unwrap_or(i64::MAX))))
}

/// `getContentAddress(location)`: what the storage names the content by, for a reader to pin
/// (`ResourceFacade::content_address`): `{kind, id, size, tag}`, or null where it has no such address (a file on disk)
/// or nothing is there.
fn files_content_address(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let Some(address) = with_resources(|resources| resources.content_address(&location))? else {
        return Ok(Ret::Value(Value::Null));
    };
    let mut map = indexmap::IndexMap::default();
    map.insert("kind".to_string(), Value::string(address.kind.name()));
    map.insert("id".to_string(), Value::string(address.id));
    map.insert(
        "size".to_string(),
        Value::I64(i64::try_from(address.size).unwrap_or(i64::MAX)),
    );
    map.insert("tag".to_string(), Value::string(address.tag));
    Ok(Ret::Value(Value::Map(map)))
}

/// A PNG thumbnail of the image at `source`, no side longer than `max_size`, written to `target`. Null when the bytes
/// are not an image this build decodes.
fn files_thumbnail(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let source = call.text_or_empty(0);
    let target = call.text_or_empty(1);
    let max_size = call.number(2).unwrap_or_default();
    let bytes = with_resources(|resources| resources.get(&source)?.read_bytes())?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let max = if max_size.is_finite() && max_size >= 1.0 {
        (max_size as u32).min(4096)
    } else {
        256
    };
    let Some(Thumbnail { width, height, png }) = thumbnail_png(&bytes, max) else {
        return Ok(Ret::Value(Value::Null));
    };
    with_resources(|resources| resources.write_bytes(&target, &png))?;
    let mut map = indexmap::IndexMap::default();
    map.insert("width".to_string(), Value::I64(i64::from(width)));
    map.insert("height".to_string(), Value::I64(i64::from(height)));
    map.insert("contentType".to_string(), Value::string("image/png"));
    Ok(Ret::Value(Value::Map(map)))
}

/// `makeStickerThumbnail` (plans/Comms-Messaging.md §18.3 phase 20): a sticker's picture checked before it is decoded,
/// so a small file that claims huge sides (a decompression bomb) is refused from its header: its format guessed from its
/// bytes (PNG, GIF or WebP only, whatever the name or the browser said), its sides at most `maxSide`, then decoded within
/// [`STICKER_MAX_ALLOC`] and its still thumbnail written to `target`.
fn files_sticker_thumbnail(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let source = call.text_or_empty(0);
    let target = call.text_or_empty(1);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let side = |number: f64, default: u32| {
        if number.is_finite() && number >= 1.0 {
            (number as u32).min(4096)
        } else {
            default
        }
    };
    let max_side = side(call.number(2).unwrap_or_default(), 512);
    let thumb_size = side(call.number(3).unwrap_or_default(), 128);
    let bytes = with_resources(|resources| resources.get(&source)?.read_bytes())?;
    let checked = sticker_thumbnail(&bytes, max_side, thumb_size);
    let mut map = indexmap::IndexMap::default();
    match checked {
        Sticker::Refused {
            reason,
            format,
            width,
            height,
        } => {
            map.insert("reasonCode".to_string(), Value::string(reason));
            map.insert("format".to_string(), format.map(Value::string).unwrap_or(Value::Null));
            map.insert(
                "width".to_string(),
                width.map_or(Value::Null, |w| Value::I64(i64::from(w))),
            );
            map.insert(
                "height".to_string(),
                height.map_or(Value::Null, |h| Value::I64(i64::from(h))),
            );
        }
        Sticker::Made {
            format,
            width,
            height,
            thumb,
        } => {
            with_resources(|resources| resources.write_bytes(&target, &thumb.png))?;
            map.insert("format".to_string(), Value::string(format));
            map.insert("contentType".to_string(), Value::string(format!("image/{format}")));
            map.insert("width".to_string(), Value::I64(i64::from(width)));
            map.insert("height".to_string(), Value::I64(i64::from(height)));
            map.insert("thumbWidth".to_string(), Value::I64(i64::from(thumb.width)));
            map.insert("thumbHeight".to_string(), Value::I64(i64::from(thumb.height)));
        }
    }
    Ok(Ret::Value(Value::Map(map)))
}

/// The memory one sticker's decode may take: a 512 px RGBA frame is 1 MiB; a GIF or WebP decodes its first frame.
#[cfg(feature = "image")]
pub(crate) const STICKER_MAX_ALLOC: u64 = 16 * 1024 * 1024;

/// What a sticker's check found.
pub(crate) enum Sticker {
    Refused {
        reason: &'static str,
        format: Option<&'static str>,
        width: Option<u32>,
        height: Option<u32>,
    },
    /// Made only with an image decoder (the `image` feature).
    #[cfg_attr(not(feature = "image"), allow(dead_code))]
    Made {
        format: &'static str,
        width: u32,
        height: u32,
        thumb: Thumbnail,
    },
}

/// A sticker's picture: its format from its bytes, its sides from its header, and only then its decode, within limits.
#[cfg(feature = "image")]
pub(crate) fn sticker_thumbnail(bytes: &[u8], max_side: u32, thumb_size: u32) -> Sticker {
    use ::image::{ImageFormat, ImageReader, Limits};
    let refused = |reason, format, width, height| Sticker::Refused {
        reason,
        format,
        width,
        height,
    };
    let reader = match ImageReader::new(Cursor::new(bytes)).with_guessed_format() {
        Ok(reader) => reader,
        Err(_) => return refused("badType", None, None, None),
    };
    let format = match reader.format() {
        Some(ImageFormat::Png) => "png",
        Some(ImageFormat::Gif) => "gif",
        Some(ImageFormat::WebP) => "webp",
        _ => return refused("badType", None, None, None),
    };
    let (width, height) = match reader.into_dimensions() {
        Ok(sides) => sides,
        Err(_) => return refused("badType", Some(format), None, None),
    };
    if width == 0 || height == 0 || width > max_side || height > max_side {
        return refused("tooLarge", Some(format), Some(width), Some(height));
    }
    let mut reader = match ImageReader::new(Cursor::new(bytes)).with_guessed_format() {
        Ok(reader) => reader,
        Err(_) => return refused("badType", Some(format), None, None),
    };
    let mut limits = Limits::default();
    limits.max_image_width = Some(max_side);
    limits.max_image_height = Some(max_side);
    limits.max_alloc = Some(STICKER_MAX_ALLOC);
    reader.limits(limits);
    let Ok(image) = reader.decode() else {
        return refused("badType", Some(format), Some(width), Some(height));
    };
    let small = if image.width() > thumb_size || image.height() > thumb_size {
        image.thumbnail(thumb_size, thumb_size)
    } else {
        image
    };
    let mut out = Cursor::new(Vec::new());
    if small.write_to(&mut out, ImageFormat::Png).is_err() {
        return refused("badType", Some(format), Some(width), Some(height));
    }
    Sticker::Made {
        format,
        width,
        height,
        thumb: Thumbnail {
            width: small.width(),
            height: small.height(),
            png: out.into_inner(),
        },
    }
}

/// No image decoder in this build (the `image` feature): no sticker.
#[cfg(not(feature = "image"))]
pub(crate) fn sticker_thumbnail(_bytes: &[u8], _max_side: u32, _thumb_size: u32) -> Sticker {
    Sticker::Refused {
        reason: "badType",
        format: None,
        width: None,
        height: None,
    }
}

/// A scaled image as PNG, with its size.
pub(crate) struct Thumbnail {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) png: Vec<u8>,
}

/// The image in `bytes` scaled to fit `max` by `max` (never up), as PNG. It is decoded within the limits of
/// `convert#Image`, so an image claiming huge sides is refused rather than allocated (SEC-SCR-09).
#[cfg(feature = "image")]
pub(crate) fn thumbnail_png(bytes: &[u8], max: u32) -> Option<Thumbnail> {
    let image = crate::service::decode_image(bytes)?;
    let small = if image.width() > max || image.height() > max {
        image.thumbnail(max, max)
    } else {
        image
    };
    let mut out = Cursor::new(Vec::new());
    small.write_to(&mut out, ::image::ImageFormat::Png).ok()?;
    Some(Thumbnail {
        width: small.width(),
        height: small.height(),
        png: out.into_inner(),
    })
}

/// No image decoder in this build (the `image` feature): no thumbnail.
#[cfg(not(feature = "image"))]
pub(crate) fn thumbnail_png(_bytes: &[u8], _max: u32) -> Option<Thumbnail> {
    None
}

/// A whole number argument from a script, clamped to `min..=max`, else `default`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn bounded(number: f64, min: usize, max: usize, default: usize) -> usize {
    if number.is_finite() && number >= 1.0 {
        (number as usize).clamp(min, max)
    } else {
        default
    }
}

/// The bytes at `location`, or the error naming it, without throwing into the script.
fn read_location(location: &str) -> Result<Result<Vec<u8>, crate::context::Error>, HostError> {
    with_ec(|ec| {
        ec.factory()
            .resource()
            .get(location)
            .and_then(|found| found.read_bytes())
    })
}

/// Log once per process that there is no PDF engine, so PDFs get no preview and no text.
fn log_no_pdf_engine() {
    static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        tracing::warn!("PDF files get no preview and no extracted text: there is no PDF engine (Chromium)");
    }
}

/// `makePdfPreview(source, target, maxSize)`: page 1 of the PDF at `source` as a PNG at `target`, no side longer than
/// `maxSize`. Null, with no error, when there is no PDF engine or the PDF does not open (logged by location).
fn files_pdf_preview(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let source = call.text_or_empty(0);
    let target = call.text_or_empty(1);
    let max_size = call.number(2).unwrap_or_default();
    if !crate::screen::PdfRenderer::available() {
        log_no_pdf_engine();
        return Ok(Ret::Value(Value::Null));
    }
    let bytes = match read_location(&source)? {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::error!(location = %source, "PDF preview not made: {err}");
            return Ok(Ret::Value(Value::Null));
        }
    };
    let max = u32::try_from(bounded(max_size, 16, 4096, 1024)).unwrap_or(1024);
    let preview = match crate::screen::PdfRenderer::preview(&bytes, true, max, 0, 0) {
        Ok(preview) if !preview.png.is_empty() => preview,
        Ok(_) => {
            tracing::error!(location = %source, "PDF preview not made: the PDF has no pages");
            return Ok(Ret::Value(Value::Null));
        }
        Err(err) => {
            tracing::error!(location = %source, "PDF preview not made: {err}");
            return Ok(Ret::Value(Value::Null));
        }
    };
    with_resources(|resources| resources.write_bytes(&target, &preview.png))?;
    let mut map = indexmap::IndexMap::default();
    map.insert("width".to_string(), Value::I64(i64::from(preview.width)));
    map.insert("height".to_string(), Value::I64(i64::from(preview.height)));
    map.insert("pages".to_string(), Value::I64(i64::from(preview.pages)));
    map.insert("contentType".to_string(), Value::string("image/png"));
    Ok(Ret::Value(Value::Map(map)))
}

/// `readSheetPreview(location, sheetName, maxRows, maxCols)`: the first rows of a sheet of the spreadsheet at
/// `location`. Null when it is not a spreadsheet this build reads (logged by location).
fn files_sheet_preview(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let sheet_name = call.text_or_empty(1);
    let max_rows = call.number(2).unwrap_or_default();
    let max_cols = call.number(3).unwrap_or_default();
    let bytes = match read_location(&location)? {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::error!(location = %location, "spreadsheet preview not made: {err}");
            return Ok(Ret::Value(Value::Null));
        }
    };
    let (max_rows, max_cols) = (bounded(max_rows, 1, 1000, 100), bounded(max_cols, 1, 200, 26));
    match sheet_preview(&bytes, &sheet_name, max_rows, max_cols) {
        Ok(preview) => Ok(Ret::Value(preview)),
        Err(err) => {
            tracing::error!(location = %location, "spreadsheet preview not made: {err}");
            Ok(Ret::Value(Value::Null))
        }
    }
}

#[cfg(feature = "xlsx")]
fn sheet_preview(bytes: &[u8], sheet_name: &str, max_rows: usize, max_cols: usize) -> Result<Value, String> {
    let preview = crate::service::spreadsheet::preview_rows(bytes, sheet_name, max_rows, max_cols)
        .map_err(|err| err.message().to_string())?;
    let mut map = indexmap::IndexMap::default();
    map.insert(
        "sheetNames".to_string(),
        Value::List(preview.sheet_names.into_iter().map(Value::String).collect()),
    );
    map.insert("sheetName".to_string(), Value::string(preview.sheet_name));
    map.insert(
        "rows".to_string(),
        Value::List(preview.rows.into_iter().map(Value::List).collect()),
    );
    map.insert("truncated".to_string(), Value::Bool(preview.truncated));
    Ok(Value::Map(map))
}

#[cfg(not(feature = "xlsx"))]
fn sheet_preview(_bytes: &[u8], _sheet_name: &str, _max_rows: usize, _max_cols: usize) -> Result<Value, String> {
    Err("this build reads no spreadsheets (the xlsx feature)".to_string())
}

/// The extensions of the spreadsheets [`files_extract_text`] and `readSheetPreview` read.
const SHEET_EXTENSIONS: [&str; 5] = ["xlsx", "xlsm", "xlsb", "xls", "ods"];
/// The pages of a PDF whose text is extracted.
const TEXT_PAGES: u32 = 50;

/// `extractText(location, cacheKey, maxChars, cacheOnly)`: the text of the PDF, spreadsheet or text file at
/// `location`, at most `maxChars`. It is kept in `runtime://tmp/file-text/<cacheKey>.txt`, or a `.none` marker when
/// the file has none or could not be read (logged by location), so it is extracted once; `clean#DriveTempFiles` removes
/// old ones. `""` when there is no text or the file is of another kind; with `cacheOnly`, null when nothing is kept yet.
fn files_extract_text(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let cache_key = call.text_or_empty(1);
    let max_chars = call.number(2).unwrap_or_default();
    let cache_only = call.bool(3).unwrap_or_default();
    if cache_key.is_empty()
        || cache_key.len() > 128
        || !cache_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(HostError::message(
            "extractText needs a cacheKey of letters, digits, - and _",
        ));
    }
    let kept = format!("runtime://tmp/file-text/{cache_key}.txt");
    let none = format!("runtime://tmp/file-text/{cache_key}.none");
    let exists = |location: &str| with_ec(|ec| ec.factory().resource().get(location).is_ok_and(|found| found.exists()));
    if exists(&kept)? {
        if let Ok(text) = read_location(&kept)? {
            return Ok(Ret::Value(Value::string(String::from_utf8_lossy(&text).into_owned())));
        }
    }
    if exists(&none)? {
        return Ok(Ret::Value(Value::string("")));
    }
    if cache_only {
        return Ok(Ret::Value(Value::Null));
    }
    let max_chars = bounded(max_chars, 1, 1_000_000, 100_000);
    let name = location.rsplit('/').next().unwrap_or("");
    let extension = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    let extracted: Result<String, String> = if extension == "pdf" {
        if crate::screen::PdfRenderer::available() {
            read_location(&location)?
                .map_err(|err| err.message().to_string())
                .and_then(|bytes| {
                    crate::screen::PdfRenderer::preview(&bytes, false, 0, TEXT_PAGES, max_chars)
                        .map(|preview| preview.text)
                        .map_err(|err| err.message().to_string())
                })
        } else {
            log_no_pdf_engine();
            Ok(String::new())
        }
    } else if SHEET_EXTENSIONS.contains(&extension.as_str()) {
        read_location(&location)?
            .map_err(|err| err.message().to_string())
            .and_then(|bytes| sheet_text(&bytes, max_chars))
    } else if moqui_lib::resource::content_type_for_name(name).is_some_and(moqui_lib::resource::is_text_content_type) {
        read_location(&location)?
            .map_err(|err| err.message().to_string())
            .map(|bytes| String::from_utf8_lossy(&bytes).chars().take(max_chars).collect())
    } else {
        return Ok(Ret::Value(Value::string("")));
    };
    let text = match extracted {
        Ok(text) => text.trim().to_string(),
        Err(err) => {
            tracing::error!(location = %location, "text not extracted: {err}");
            String::new()
        }
    };
    let (target, bytes) = if text.is_empty() {
        (&none, Vec::new())
    } else {
        (&kept, text.as_bytes().to_vec())
    };
    if let Err(err) = with_ec(|ec| ec.factory().resource().write_bytes(target, &bytes))? {
        tracing::warn!(location = %location, "extracted text not kept at {target}: {err}");
    }
    Ok(Ret::Value(Value::string(text)))
}

#[cfg(feature = "xlsx")]
fn sheet_text(bytes: &[u8], max_chars: usize) -> Result<String, String> {
    crate::service::spreadsheet::sheet_text(bytes, max_chars).map_err(|err| err.message().to_string())
}

#[cfg(not(feature = "xlsx"))]
fn sheet_text(_bytes: &[u8], _max_chars: usize) -> Result<String, String> {
    Err("this build reads no spreadsheets (the xlsx feature)".to_string())
}

/// A zip of the files at the entries' locations, each under its `path`, written to `target`. An entry that cannot be
/// read is reported and left out; the others are still written.
/// `writeZip(target, entries)`: the zip is built in a file under `runtime/tmp/drive-zip/.building`, each entry copied
/// from its source a window at a time, so neither a file nor the archive is ever whole in memory (SEC-BOS-83: a share
/// link's folder zip). A `runtime://` target is the part file renamed into place, so a reader never sees a part-written
/// zip and nothing is copied; another target is written from the file. The part file never stays. A file that cannot
/// be read is left out and reported.
fn files_write_zip(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let target = call.text_or_empty(0);
    let files: Vec<(String, String)> = entries_of(call.value_or_null(1))
        .iter()
        .map(|item| (text_field(item, "path"), text_field(item, "location")))
        .collect();
    // the part files' folder, through the facade's resolution of runtime://
    let building = with_resources(|resources| resources.get(BUILDING))?
        .file_path()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| HostError::message(format!("writing the zip: {BUILDING} is not a folder on disk")))?;
    let (count, size, failures) = with_part_file(&building, |part| {
        let (count, failures) = write_zip_file(part, files, |location| {
            match with_ec(|ec| ec.factory().resource().open(location)) {
                Ok(Ok(found)) => Ok(found.map(|opened| (opened.stat.size, opened.source))),
                Ok(Err(err)) => Err(err.message().to_string()),
                Err(_) => Err(format!("no execution context to read {location}")),
            }
        })?;
        let size = std::fs::metadata(part).map_err(|err| zip_error(&err))?.len();
        let name = part.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        deliver_zip(part, &format!("{BUILDING}/{name}"), &target, size)?;
        Ok((count, size, failures))
    })?;
    let mut map = indexmap::IndexMap::default();
    map.insert("count".to_string(), Value::I64(count));
    map.insert("size".to_string(), Value::I64(i64::try_from(size).unwrap_or(i64::MAX)));
    map.insert(
        "failures".to_string(),
        Value::List(
            failures
                .into_iter()
                .map(|(path, error)| {
                    let mut failure = indexmap::IndexMap::default();
                    failure.insert("path".to_string(), Value::string(path));
                    failure.insert("error".to_string(), Value::string(error));
                    Value::Map(failure)
                })
                .collect(),
        ),
    );
    Ok(Ret::Value(Value::Map(map)))
}

fn zip_error(err: &dyn std::fmt::Display) -> HostError {
    HostError::message(format!("writing the zip: {err}"))
}

/// Run `body` with a new part file's path under `dir` (made when missing), and remove the part file afterwards whatever
/// the body did, unless the body moved it away.
fn with_part_file<T>(
    dir: &std::path::Path,
    body: impl FnOnce(&std::path::Path) -> Result<T, HostError>,
) -> Result<T, HostError> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    std::fs::create_dir_all(dir).map_err(|err| zip_error(&err))?;
    let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let part = dir.join(format!("{}-{number}.part", std::process::id()));
    let result = body(&part);
    if let Err(err) = std::fs::remove_file(&part) {
        if err.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(part = %part.display(), "a zip's part file was not removed: {err}");
        }
    }
    result
}

/// Where a zip's part files are built: under the runtime directory, which `clean#DriveTempFiles` sweeps.
const BUILDING: &str = "runtime://tmp/drive-zip/.building";

/// The built zip at `part` (`part_location` its `runtime://` location) to `target`: a `runtime://` target by the
/// resource facade's rename, which resolves both paths and keeps its checks (a `#` refused, the effect fence, a WebDAV
/// lock on the target); anything else, or a rename that fails, written from the file.
fn deliver_zip(part: &std::path::Path, part_location: &str, target: &str, size: u64) -> Result<(), HostError> {
    if target.starts_with("runtime://") {
        match with_resources(|resources| resources.rename(part_location, target)) {
            Ok(()) => return Ok(()),
            Err(err) => tracing::warn!(target = %target, "a zip was not renamed into place, so it is written: {err:?}"),
        }
    }
    let mut file = std::fs::File::open(part).map_err(|err| zip_error(&err))?;
    with_resources(|resources| resources.write_from(target, &mut file, Some(size)))
}

/// Whether an entry of `size` bytes needs zip64: past `u32::MAX`, the size a plain entry header holds.
fn needs_zip64(size: u64) -> bool {
    size > u64::from(u32::MAX)
}

/// A source to read into a zip: its size and bytes, `None` when nothing is at the location, or an error.
type ZipSource = Result<Option<(u64, std::sync::Arc<dyn crate::resource::source::ByteSource>)>, String>;

/// The zip of `files` (`(path in the zip, location)`) written to the file at `part`, each source `open` gives read as a
/// stream: the entries written and the `(path, error)` of each left out (a bad or repeated path, a file that is not
/// there or cannot be read, whole or part way, its part taken back out of the zip).
fn write_zip_file(
    part: &std::path::Path,
    files: Vec<(String, String)>,
    mut open: impl FnMut(&str) -> ZipSource,
) -> Result<(i64, Vec<(String, String)>), HostError> {
    let file = std::fs::File::create(part).map_err(|err| zip_error(&err))?;
    let mut writer = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut failures = Vec::new();
    let mut count = 0i64;
    let mut seen = std::collections::HashSet::new();
    for (path, location) in files {
        let path = path.trim_matches('/').to_string();
        if path.is_empty() || path.split('/').any(|part| part == ".." || part == ".") || !seen.insert(path.clone()) {
            failures.push((path, "bad or repeated path in the zip".to_string()));
            continue;
        }
        let (size, source) = match open(&location) {
            Ok(Some(opened)) => opened,
            Ok(None) => {
                failures.push((path, format!("no file at {location}")));
                continue;
            }
            Err(err) => {
                tracing::error!(location = %location, "cannot read a file for a zip: {err}");
                failures.push((path, err));
                continue;
            }
        };
        writer
            .start_file(path.as_str(), options.large_file(needs_zip64(size)))
            .map_err(|err| zip_error(&err))?;
        let mut reader = crate::resource::source::SourceReader::new(source);
        match std::io::copy(&mut reader, &mut writer) {
            Ok(_) => count += 1,
            Err(err) => {
                tracing::error!(location = %location, "cannot read a file for a zip: {err}");
                writer.abort_file().map_err(|err| zip_error(&err))?;
                failures.push((path, err.to_string()));
            }
        }
    }
    writer
        .finish()
        .map_err(|err| zip_error(&err))?
        .into_inner()
        .map_err(|err| zip_error(&err))?
        .sync_all()
        .map_err(|err| zip_error(&err))?;
    Ok((count, failures))
}

type GrantMac = Hmac<Sha256>;

fn grant_tag(key: &str, subject: &str, expires: i64) -> Option<String> {
    let mut mac = GrantMac::new_from_slice(key.as_bytes()).ok()?;
    mac.update(subject.as_bytes());
    mac.update(b"\n");
    mac.update(expires.to_string().as_bytes());
    Some(
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

/// A grant for `subject` that `check_grant` accepts with the same key until `ttl_seconds` have passed:
/// `<expiry millis>.<HMAC-SHA256 hex of subject and expiry>`. The key is a secret the holder of the grant never sees.
pub(crate) fn make_grant(key: &str, subject: &str, ttl_seconds: i64) -> Option<String> {
    if key.is_empty() {
        return None;
    }
    let expires = crate::user::now_millis().saturating_add(ttl_seconds.clamp(1, 7 * 86_400).saturating_mul(1000));
    grant_tag(key, subject, expires).map(|tag| format!("{expires}.{tag}"))
}

/// Whether `token` is a grant for `subject` with `key` that has not expired. Compared in constant time.
pub(crate) fn check_grant(key: &str, subject: &str, token: &str) -> bool {
    let Some((expires, tag)) = token.split_once('.') else {
        return false;
    };
    let Ok(expires) = expires.parse::<i64>() else {
        return false;
    };
    if key.is_empty() || expires < crate::user::now_millis() {
        return false;
    }
    let Some(expected) = grant_tag(key, subject, expires) else {
        return false;
    };
    expected.len() == tag.len()
        && expected
            .bytes()
            .zip(tag.bytes())
            .fold(0u8, |diff, (left, right)| diff | (left ^ right))
            == 0
}

fn files_make_grant(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let key = call.text_or_empty(0);
    let subject = call.text_or_empty(1);
    let ttl_seconds = call.number(2).unwrap_or_default();
    #[allow(clippy::cast_possible_truncation)]
    let ttl = if ttl_seconds.is_finite() {
        ttl_seconds as i64
    } else {
        3600
    };
    make_grant(&key, &subject, ttl)
        .map(Ret::from)
        .ok_or_else(|| HostError::message("a grant needs a key"))
}

fn files_check_grant(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let key = call.text_or_empty(0);
    let subject = call.text_or_empty(1);
    let token = call.text_or_empty(2);
    Ok(check_grant(&key, &subject, &token).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source whose bytes fail part way, as a file whose storage goes away while it is read.
    struct FailingSource;

    impl crate::resource::source::ByteSource for FailingSource {
        fn len(&self) -> u64 {
            4 * 1024 * 1024
        }

        fn read_at(&self, offset: u64, max: usize) -> Result<Vec<u8>, moqui_lib::error::Error> {
            if offset == 0 {
                Ok(vec![7; max.min(1024 * 1024)])
            } else {
                Err(moqui_lib::error::Error::new("the storage went away"))
            }
        }
    }

    /// A new empty directory of its own under the system temp directory, removed when dropped.
    struct ScratchDir(std::path::PathBuf);

    impl ScratchDir {
        fn new(label: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default();
            let path = std::env::temp_dir().join(format!("moqui-zip-{label}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn memory(bytes: &[u8]) -> ZipSource {
        let source: std::sync::Arc<dyn crate::resource::source::ByteSource> =
            std::sync::Arc::new(crate::resource::source::MemorySource::new(bytes.to_vec()));
        Ok(Some((bytes.len() as u64, source)))
    }

    /// SEC-BOS-83: a file that fails part way through the list is left out and reported, its part taken back out, and
    /// the files before and after it are in the zip, which is a whole zip.
    #[test]
    fn a_zip_leaves_out_a_file_that_fails_mid_list_and_keeps_the_others() {
        let dir = ScratchDir::new("mid-list");
        let part = dir.0.join("one.part");
        let files = vec![
            ("a.txt".to_string(), "mem:a".to_string()),
            ("b.bin".to_string(), "fail:b".to_string()),
            ("missing.txt".to_string(), "none:c".to_string()),
            ("c.txt".to_string(), "mem:c".to_string()),
            ("../out.txt".to_string(), "mem:a".to_string()),
        ];
        let (count, failures) = write_zip_file(&part, files, |location| match location {
            "mem:a" => memory(b"alpha"),
            "mem:c" => memory(b"gamma"),
            "fail:b" => {
                let source: std::sync::Arc<dyn crate::resource::source::ByteSource> =
                    std::sync::Arc::new(FailingSource);
                Ok(Some((4 * 1024 * 1024, source)))
            }
            _ => Ok(None),
        })
        .unwrap();
        assert_eq!(count, 2);
        let failed: Vec<&str> = failures.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(failed, ["b.bin", "missing.txt", "../out.txt"]);
        assert!(failures[0].1.contains("the storage went away"), "{failures:?}");
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&part).unwrap()).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().unwrap().to_string())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.contains(&"a.txt".to_string()) && names.contains(&"c.txt".to_string()));
        let mut text = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("c.txt").unwrap(), &mut text).unwrap();
        assert_eq!(text, "gamma");
    }

    #[test]
    fn an_entry_past_four_gibibytes_takes_zip64() {
        assert!(!needs_zip64(u64::from(u32::MAX)));
        assert!(needs_zip64(u64::from(u32::MAX) + 1));
    }

    /// The part file never stays: removed after a body that fails as after one that succeeds.
    #[test]
    fn a_part_file_is_removed_whatever_the_build_did() {
        let dir = ScratchDir::new("part");
        let mut seen = None;
        let failed: Result<(), HostError> = with_part_file(&dir.0, |part| {
            std::fs::write(part, b"half").unwrap();
            seen = Some(part.to_path_buf());
            Err(HostError::message("the build failed"))
        });
        assert!(failed.is_err());
        assert!(!seen.unwrap().exists());
        let mut kept = None;
        with_part_file(&dir.0, |part| {
            std::fs::write(part, b"whole").unwrap();
            kept = Some(part.to_path_buf());
            Ok(())
        })
        .unwrap();
        assert!(!kept.unwrap().exists());
    }

    #[test]
    fn a_grant_checks_with_its_key_and_subject_only() {
        let grant = make_grant("secret", "share:1", 60).unwrap();
        assert!(check_grant("secret", "share:1", &grant));
        assert!(!check_grant("other", "share:1", &grant));
        assert!(!check_grant("secret", "share:2", &grant));
        let (expires, tag) = grant.split_once('.').unwrap();
        let later: i64 = expires.parse::<i64>().unwrap() + 1000;
        assert!(
            !check_grant("secret", "share:1", &format!("{later}.{tag}")),
            "a changed expiry fails"
        );
        let past = crate::user::now_millis() - 1000;
        let stale = format!("{past}.{}", grant_tag("secret", "share:1", past).unwrap());
        assert!(!check_grant("secret", "share:1", &stale), "an expired grant fails");
        assert!(make_grant("", "share:1", 60).is_none());
        assert!(!check_grant("secret", "share:1", "nonsense"));
    }

    /// The picture in `format` of a `width` by `height` solid image.
    #[cfg(feature = "image")]
    fn picture(width: u32, height: u32, format: ::image::ImageFormat) -> Vec<u8> {
        let image = ::image::RgbImage::from_pixel(width, height, ::image::Rgb([200, 30, 30]));
        let mut out = Cursor::new(Vec::new());
        ::image::DynamicImage::ImageRgb8(image)
            .write_to(&mut out, format)
            .unwrap();
        out.into_inner()
    }

    /// A sticker's picture (plans/Comms-Messaging.md §18.3 phase 20): its type from its bytes, its sides from its header
    /// before any decode (a tiny PNG claiming 100,000 px a side is refused unread), within the sticker's limits.
    #[cfg(feature = "image")]
    #[test]
    fn a_sticker_is_checked_by_its_bytes_and_its_header_before_it_is_decoded() {
        let Sticker::Made {
            format,
            width,
            height,
            thumb,
        } = sticker_thumbnail(&picture(300, 150, ::image::ImageFormat::Png), 512, 128)
        else {
            panic!("a PNG sticker")
        };
        assert_eq!((format, width, height), ("png", 300, 150));
        assert_eq!((thumb.width, thumb.height), (128, 64));
        assert!(matches!(
            sticker_thumbnail(&picture(8, 8, ::image::ImageFormat::Gif), 512, 128),
            Sticker::Made { format: "gif", .. }
        ));
        // a JPEG or a BMP, whatever it is named, is no sticker
        for format in [::image::ImageFormat::Jpeg, ::image::ImageFormat::Bmp] {
            assert!(matches!(
                sticker_thumbnail(&picture(8, 8, format), 512, 128),
                Sticker::Refused { reason: "badType", .. }
            ));
        }
        assert!(matches!(
            sticker_thumbnail(b"<svg xmlns='http://www.w3.org/2000/svg'/>", 512, 128),
            Sticker::Refused { reason: "badType", .. }
        ));
        assert!(matches!(
            sticker_thumbnail(&picture(600, 10, ::image::ImageFormat::Png), 512, 128),
            Sticker::Refused {
                reason: "tooLarge",
                width: Some(600),
                ..
            }
        ));
        // a decompression bomb: a header claiming 100,000 by 100,000 px and almost no data, refused from its header
        let mut bomb = picture(1, 1, ::image::ImageFormat::Png);
        bomb[16..20].copy_from_slice(&100_000_u32.to_be_bytes());
        bomb[20..24].copy_from_slice(&100_000_u32.to_be_bytes());
        let crc = bomb[12..29].iter().fold(0xffff_ffff_u32, |crc, byte| {
            (0..8).fold(crc ^ u32::from(*byte), |c, _| {
                if c & 1 == 1 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                }
            })
        }) ^ 0xffff_ffff;
        bomb[29..33].copy_from_slice(&crc.to_be_bytes());
        assert!(bomb.len() < 200, "{}", bomb.len());
        assert!(matches!(
            sticker_thumbnail(&bomb, 512, 128),
            Sticker::Refused {
                reason: "tooLarge",
                width: Some(100_000),
                ..
            }
        ));
    }

    #[cfg(feature = "image")]
    #[test]
    fn a_thumbnail_fits_the_box_and_keeps_the_aspect() {
        let image = ::image::RgbImage::from_pixel(400, 200, ::image::Rgb([10, 120, 200]));
        let mut png = Cursor::new(Vec::new());
        ::image::DynamicImage::ImageRgb8(image)
            .write_to(&mut png, ::image::ImageFormat::Png)
            .unwrap();
        let Thumbnail { width, height, png } = thumbnail_png(&png.into_inner(), 100).unwrap();
        assert_eq!((width, height), (100, 50));
        assert!(png.starts_with(b"\x89PNG"));
        assert!(thumbnail_png(b"not an image", 100).is_none());
    }

    #[cfg(feature = "image")]
    #[test]
    fn a_thumbnail_is_not_made_of_an_image_with_a_side_over_the_decode_limit() {
        let image = ::image::GrayImage::from_pixel(30_001, 1, ::image::Luma([0]));
        let mut png = Cursor::new(Vec::new());
        ::image::DynamicImage::ImageLuma8(image)
            .write_to(&mut png, ::image::ImageFormat::Png)
            .unwrap();
        assert!(thumbnail_png(&png.into_inner(), 100).is_none());
    }
}
