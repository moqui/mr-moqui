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

//! The parts of `ec.resource` that drives and file browsers use (`org.moqui.impl.FileServices`): change sets, version
//! notes, last changes and change logs, the bytes under a folder, image thumbnails, PDF and spreadsheet previews, the
//! text of a file for search, zip archives, and signed grants. Guide: `docs/Revision-Management.md`.
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
//! ec.resource.makeThumbnail(source, target, maxSize) -> {width, height, contentType} or null (not an image)
//! ec.resource.makePdfPreview(source, target, maxSize) -> {width, height, pages, contentType} or null (no PDF engine,
//!                                             or the PDF does not open: logged)
//! ec.resource.readSheetPreview(location, sheetName, maxRows, maxCols)
//!                                          -> {sheetNames, sheetName, rows, truncated} or null (not a spreadsheet)
//! ec.resource.extractText(location, cacheKey, maxChars, cacheOnly) -> the text of a PDF, spreadsheet or text file,
//!                                             kept in runtime://tmp/file-text/<cacheKey>.txt; "" when it has none;
//!                                             null with cacheOnly when it was not extracted yet
//! ec.resource.writeZip(target, [{path, location}]) -> {count, size, failures: [{path, error}]}
//! ec.resource.makeGrant(key, subject, ttlSeconds)  -> token;  ec.resource.checkGrant(key, subject, token) -> boolean
//! ```

use std::io::{Cursor, Write};

use hmac::{Hmac, Mac};
use sha2::Sha256;

use super::*;
use crate::context::{ResourceChange, VersionAuthor};

const FILES_JS: &str = r#"(function (ec, host) {
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
    ec.resource.makeThumbnail = (source, target, maxSize) =>
        host.filesThumbnail(String(source), String(target), maxSize == null ? 256 : Number(maxSize));
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
    ec.resource.makeGrant = (key, subject, ttlSeconds) =>
        host.filesMakeGrant(String(key), String(subject), ttlSeconds == null ? 3600 : Number(ttlSeconds));
    ec.resource.checkGrant = (key, subject, token) =>
        token == null ? false : host.filesCheckGrant(String(key), String(subject), String(token));
})"#;

pub(super) fn install(ctx: &Ctx<'_>) -> Result<(), Error> {
    let host = Object::new(ctx.clone()).map_err(js_error)?;
    macro_rules! method {
        ($name:literal, $function:expr) => {
            let function = caught(ctx, $name, Function::new(ctx.clone(), $function))?;
            caught(ctx, $name, host.set($name, function))?;
        };
    }
    method!("filesApplyChangeSet", files_apply_change_set);
    method!("filesPutContent", files_put_content);
    method!("filesLastChange", files_last_change);
    method!("filesChildChanges", files_child_changes);
    method!("filesChangeLog", files_change_log);
    method!("filesTreeSize", files_tree_size);
    method!("filesThumbnail", files_thumbnail);
    method!("filesPdfPreview", files_pdf_preview);
    method!("filesSheetPreview", files_sheet_preview);
    method!("filesExtractText", files_extract_text);
    method!("filesWriteZip", files_write_zip);
    method!("filesRestoreVersion", files_restore_version);
    method!("filesVersionBytes", files_version_bytes);
    method!("filesContentType", files_content_type);
    method!("filesMakeGrant", files_make_grant);
    method!("filesCheckGrant", files_check_grant);
    let define: Function = eval_js(ctx, FILES_JS)?;
    let ec: Object = caught(ctx, "ec", ctx.globals().get("ec"))?;
    caught(ctx, "files", define.call::<_, ()>((ec, host)))
}

fn with_resources<T>(
    ctx: &Ctx<'_>,
    body: impl FnOnce(&crate::context::ResourceFacade) -> Result<T, crate::context::Error>,
) -> rquickjs::Result<T> {
    with_ec(ctx, |ec| body(ec.factory().resource()))?.map_err(|err| throw_context(ctx, &err))
}

/// The bytes of a change's content as the script passed it: text, binary, an uploaded file or a stream. Anything else
/// (an object or a list, such as a stream that lost its bytes on the way) is refused, never written as its text.
fn content_bytes(ctx: &Ctx<'_>, value: &JsValue<'_>) -> rquickjs::Result<Vec<u8>> {
    if let Some(object) = value.as_object() {
        if let Some(class) = object.as_class::<HostUpload>() {
            return class
                .borrow()
                .inner
                .read_bytes()
                .map_err(|err| Exception::throw_message(ctx, err.message()));
        }
        if let Some(class) = object.as_class::<super::upload::HostStream>() {
            return class
                .borrow()
                .inner
                .bytes()
                .map_err(|err| Exception::throw_message(ctx, err.message()));
        }
    }
    let refused = || Exception::throw_message(ctx, "a file's content is text, bytes, an uploaded file or a stream");
    match js_to_host(ctx, value)? {
        Value::Null => Ok(Vec::new()),
        Value::Bytes(bytes) => Ok(bytes),
        Value::String(text) => Ok(text.into_bytes()),
        Value::Object(object) => {
            if let Some(file) = object.downcast::<crate::web::multipart::UploadedFile>() {
                file.read_bytes()
                    .map_err(|err| Exception::throw_message(ctx, err.message()))
            } else if let Some(stream) = object.downcast::<super::upload::StreamBytes>() {
                stream
                    .bytes()
                    .map_err(|err| Exception::throw_message(ctx, err.message()))
            } else {
                Err(refused())
            }
        }
        Value::Map(_) | Value::List(_) => Err(refused()),
        other => Ok(moqui_lib::object::to_plain_string(&other).into_bytes()),
    }
}

fn text_field(ctx: &Ctx<'_>, object: &Object<'_>, name: &str) -> rquickjs::Result<String> {
    let value: JsValue = object.get(name)?;
    match js_to_host(ctx, &value)? {
        Value::Null => Ok(String::new()),
        value => Ok(moqui_lib::object::to_plain_string(&value)),
    }
}

/// The `author(userId, date)` of a change set builder as a version author: the date a timestamp or milliseconds.
fn version_author_of(ctx: &Ctx<'_>, value: Option<&JsValue<'_>>) -> rquickjs::Result<Option<VersionAuthor>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Value::Map(map) = js_to_host(ctx, value)? else {
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
                return Err(Exception::throw_message(
                    ctx,
                    "a change set author's date is a timestamp or milliseconds",
                ))
            }
        },
    };
    Ok((user_id.is_some() || date.is_some()).then_some(VersionAuthor { user_id, date }))
}

/// `commit(message)` of a change set builder: `{versionName, applied, failures: [{path, error}]}`.
fn files_apply_change_set<'js>(
    ctx: Ctx<'js>,
    root: String,
    list: rquickjs::Array<'js>,
    message: Opt<JsValue<'js>>,
    author: Opt<JsValue<'js>>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let mut changes = Vec::with_capacity(list.len());
        for item in list.iter::<Object>() {
            let item = item?;
            let op = text_field(&ctx, &item, "op")?;
            changes.push(match op.as_str() {
                "write" => {
                    let content: JsValue = item.get("content")?;
                    ResourceChange::Write {
                        path: text_field(&ctx, &item, "path")?,
                        bytes: content_bytes(&ctx, &content)?,
                    }
                }
                "delete" => ResourceChange::Delete {
                    path: text_field(&ctx, &item, "path")?,
                },
                "mkdir" => ResourceChange::MakeDirectory {
                    path: text_field(&ctx, &item, "path")?,
                },
                "move" => ResourceChange::Move {
                    from: text_field(&ctx, &item, "from")?,
                    to: text_field(&ctx, &item, "to")?,
                },
                other => return Err(Exception::throw_message(&ctx, &format!("unknown change {other}"))),
            });
        }
        let message = match message.0 {
            Some(value) => match js_to_host(&ctx, &value)? {
                Value::Null => None,
                value => Some(moqui_lib::object::to_plain_string(&value)),
            },
            None => None,
        };
        let author = version_author_of(&ctx, author.0.as_ref())?;
        let outcome = with_resources(&ctx, |resources| {
            crate::context::with_version_author(author, || {
                resources.apply_change_set(&root, &changes, message.as_deref())
            })
        })?;
        let mut map = indexmap::IndexMap::new();
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
                        let mut failure = indexmap::IndexMap::new();
                        failure.insert("path".to_string(), Value::string(path));
                        failure.insert("error".to_string(), Value::string(error));
                        Value::Map(failure)
                    })
                    .collect(),
            ),
        );
        host_to_js(&ctx, Value::Map(map))
    })
}

/// `putContent(location, content, note)` and the `put*` methods of a reference given a note.
fn files_put_content<'js>(
    ctx: Ctx<'js>,
    location: String,
    content: JsValue<'js>,
    note: Opt<JsValue<'js>>,
) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let bytes = content_bytes(&ctx, &content)?;
        let note = match note.0 {
            Some(value) => match js_to_host(&ctx, &value)? {
                Value::Null => None,
                value => Some(moqui_lib::object::to_plain_string(&value)),
            },
            None => None,
        };
        with_resources(&ctx, |resources| {
            resources.write_bytes_noted(&location, &bytes, note.as_deref())
        })
    })
}

fn files_last_change<'js>(ctx: Ctx<'js>, location: String) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let change = with_resources(&ctx, |resources| resources.last_change(&location))?;
        host_to_js(&ctx, change.map_or(Value::Null, |change| change.to_value()))
    })
}

fn files_child_changes<'js>(ctx: Ctx<'js>, location: String) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let changes = with_resources(&ctx, |resources| resources.child_changes(&location))?;
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
        host_to_js(&ctx, Value::List(list))
    })
}

fn files_change_log<'js>(ctx: Ctx<'js>, location: String, limit: f64) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let limit = if limit.is_finite() && limit > 0.0 {
            limit as usize
        } else {
            100
        };
        let entries = with_resources(&ctx, |resources| resources.change_log(&location, limit))?;
        let list = entries
            .into_iter()
            .map(|entry| {
                let version = entry.version;
                let mut map = indexmap::IndexMap::new();
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
        host_to_js(&ctx, Value::List(list))
    })
}

/// `restoreVersion(location, versionName, note)`: that version written back as the newest; the new version's name.
fn files_restore_version<'js>(
    ctx: Ctx<'js>,
    location: String,
    version_name: String,
    note: Opt<JsValue<'js>>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let note = match note.0 {
            Some(value) => match js_to_host(&ctx, &value)? {
                Value::Null => None,
                value => Some(moqui_lib::object::to_plain_string(&value)),
            },
            None => None,
        };
        let made = with_resources(&ctx, |resources| {
            resources.restore_version(&location, &version_name, note.as_deref())
        })?;
        host_to_js(&ctx, made.map_or(Value::Null, Value::string))
    })
}

/// `getVersionBytes(location, versionName)`: the bytes of a version as a `Uint8Array`, or null.
fn files_version_bytes<'js>(ctx: Ctx<'js>, location: String, version_name: String) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let bytes = with_resources(&ctx, |resources| resources.read_version(&location, &version_name))?;
        host_to_js(&ctx, bytes.map_or(Value::Null, Value::Bytes))
    })
}

/// `contentTypeOf(filename)`: the content type of a file name's extension, or null.
fn files_content_type<'js>(ctx: Ctx<'js>, filename: String) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        host_to_js(
            &ctx,
            moqui_lib::resource::content_type_for_name(&filename).map_or(Value::Null, Value::string),
        )
    })
}

fn files_tree_size<'js>(ctx: Ctx<'js>, location: String) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let size = with_resources(&ctx, |resources| resources.tree_size(&location))?;
        i64_to_js(&ctx, i64::try_from(size).unwrap_or(i64::MAX))
    })
}

/// A PNG thumbnail of the image at `source`, no side longer than `max_size`, written to `target`. Null when the bytes
/// are not an image this build decodes.
fn files_thumbnail<'js>(
    ctx: Ctx<'js>,
    source: String,
    target: String,
    max_size: f64,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let bytes = with_resources(&ctx, |resources| resources.get(&source)?.read_bytes())?;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let max = if max_size.is_finite() && max_size >= 1.0 {
            (max_size as u32).min(4096)
        } else {
            256
        };
        let Some(png) = thumbnail_png(&bytes, max) else {
            return Null.into_js(&ctx);
        };
        let (width, height, data) = png;
        with_resources(&ctx, |resources| resources.write_bytes(&target, &data))?;
        let mut map = indexmap::IndexMap::new();
        map.insert("width".to_string(), Value::I64(i64::from(width)));
        map.insert("height".to_string(), Value::I64(i64::from(height)));
        map.insert("contentType".to_string(), Value::string("image/png"));
        host_to_js(&ctx, Value::Map(map))
    })
}

/// The image in `bytes` scaled to fit `max` by `max` (never up), as PNG: `(width, height, png)`.
#[cfg(feature = "image")]
pub(crate) fn thumbnail_png(bytes: &[u8], max: u32) -> Option<(u32, u32, Vec<u8>)> {
    let image = ::image::load_from_memory(bytes).ok()?;
    let small = if image.width() > max || image.height() > max {
        image.thumbnail(max, max)
    } else {
        image
    };
    let mut out = Cursor::new(Vec::new());
    small.write_to(&mut out, ::image::ImageFormat::Png).ok()?;
    Some((small.width(), small.height(), out.into_inner()))
}

/// No image decoder in this build (the `image` feature): no thumbnail.
#[cfg(not(feature = "image"))]
pub(crate) fn thumbnail_png(_bytes: &[u8], _max: u32) -> Option<(u32, u32, Vec<u8>)> {
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
fn read_location(ctx: &Ctx<'_>, location: &str) -> rquickjs::Result<Result<Vec<u8>, crate::context::Error>> {
    with_ec(ctx, |ec| {
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
fn files_pdf_preview<'js>(
    ctx: Ctx<'js>,
    source: String,
    target: String,
    max_size: f64,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        if !crate::screen::PdfRenderer::available() {
            log_no_pdf_engine();
            return Null.into_js(&ctx);
        }
        let bytes = match read_location(&ctx, &source)? {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::error!(location = %source, "PDF preview not made: {err}");
                return Null.into_js(&ctx);
            }
        };
        let max = u32::try_from(bounded(max_size, 16, 4096, 1024)).unwrap_or(1024);
        let preview = match crate::screen::PdfRenderer::preview(&bytes, true, max, 0, 0) {
            Ok(preview) if !preview.png.is_empty() => preview,
            Ok(_) => {
                tracing::error!(location = %source, "PDF preview not made: the PDF has no pages");
                return Null.into_js(&ctx);
            }
            Err(err) => {
                tracing::error!(location = %source, "PDF preview not made: {err}");
                return Null.into_js(&ctx);
            }
        };
        with_resources(&ctx, |resources| resources.write_bytes(&target, &preview.png))?;
        let mut map = indexmap::IndexMap::new();
        map.insert("width".to_string(), Value::I64(i64::from(preview.width)));
        map.insert("height".to_string(), Value::I64(i64::from(preview.height)));
        map.insert("pages".to_string(), Value::I64(i64::from(preview.pages)));
        map.insert("contentType".to_string(), Value::string("image/png"));
        host_to_js(&ctx, Value::Map(map))
    })
}

/// `readSheetPreview(location, sheetName, maxRows, maxCols)`: the first rows of a sheet of the spreadsheet at
/// `location`. Null when it is not a spreadsheet this build reads (logged by location).
fn files_sheet_preview<'js>(
    ctx: Ctx<'js>,
    location: String,
    sheet_name: String,
    max_rows: f64,
    max_cols: f64,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let bytes = match read_location(&ctx, &location)? {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::error!(location = %location, "spreadsheet preview not made: {err}");
                return Null.into_js(&ctx);
            }
        };
        let (max_rows, max_cols) = (bounded(max_rows, 1, 1000, 100), bounded(max_cols, 1, 200, 26));
        match sheet_preview(&bytes, &sheet_name, max_rows, max_cols) {
            Ok(preview) => host_to_js(&ctx, preview),
            Err(err) => {
                tracing::error!(location = %location, "spreadsheet preview not made: {err}");
                Null.into_js(&ctx)
            }
        }
    })
}

#[cfg(feature = "xlsx")]
fn sheet_preview(bytes: &[u8], sheet_name: &str, max_rows: usize, max_cols: usize) -> Result<Value, String> {
    let preview = crate::service::spreadsheet::preview_rows(bytes, sheet_name, max_rows, max_cols)
        .map_err(|err| err.message().to_string())?;
    let mut map = indexmap::IndexMap::new();
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
fn files_extract_text<'js>(
    ctx: Ctx<'js>,
    location: String,
    cache_key: String,
    max_chars: f64,
    cache_only: bool,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        if cache_key.is_empty()
            || cache_key.len() > 128
            || !cache_key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(Exception::throw_message(
                &ctx,
                "extractText needs a cacheKey of letters, digits, - and _",
            ));
        }
        let kept = format!("runtime://tmp/file-text/{cache_key}.txt");
        let none = format!("runtime://tmp/file-text/{cache_key}.none");
        let exists = |location: &str| {
            with_ec(&ctx, |ec| {
                ec.factory().resource().get(location).is_ok_and(|found| found.exists())
            })
        };
        if exists(&kept)? {
            if let Ok(text) = read_location(&ctx, &kept)? {
                return host_to_js(&ctx, Value::string(String::from_utf8_lossy(&text).into_owned()));
            }
        }
        if exists(&none)? {
            return host_to_js(&ctx, Value::string(""));
        }
        if cache_only {
            return Null.into_js(&ctx);
        }
        let max_chars = bounded(max_chars, 1, 1_000_000, 100_000);
        let name = location.rsplit('/').next().unwrap_or("");
        let extension = name
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
            .unwrap_or_default();
        let extracted: Result<String, String> = if extension == "pdf" {
            if crate::screen::PdfRenderer::available() {
                read_location(&ctx, &location)?
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
            read_location(&ctx, &location)?
                .map_err(|err| err.message().to_string())
                .and_then(|bytes| sheet_text(&bytes, max_chars))
        } else if moqui_lib::resource::content_type_for_name(name)
            .is_some_and(moqui_lib::resource::is_text_content_type)
        {
            read_location(&ctx, &location)?
                .map_err(|err| err.message().to_string())
                .map(|bytes| String::from_utf8_lossy(&bytes).chars().take(max_chars).collect())
        } else {
            return host_to_js(&ctx, Value::string(""));
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
        if let Err(err) = with_ec(&ctx, |ec| ec.factory().resource().write_bytes(target, &bytes))? {
            tracing::warn!(location = %location, "extracted text not kept at {target}: {err}");
        }
        host_to_js(&ctx, Value::string(text))
    })
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
fn files_write_zip<'js>(
    ctx: Ctx<'js>,
    target: String,
    entries: rquickjs::Array<'js>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let mut files = Vec::with_capacity(entries.len());
        for item in entries.iter::<Object>() {
            let item = item?;
            files.push((text_field(&ctx, &item, "path")?, text_field(&ctx, &item, "location")?));
        }
        let mut failures = Vec::new();
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut count = 0i64;
        let mut seen = std::collections::HashSet::new();
        for (path, location) in files {
            let path = path.trim_matches('/').to_string();
            if path.is_empty() || path.split('/').any(|part| part == ".." || part == ".") || !seen.insert(path.clone())
            {
                failures.push((path, "bad or repeated path in the zip".to_string()));
                continue;
            }
            let bytes = match with_ec(&ctx, |ec| {
                ec.factory()
                    .resource()
                    .get(&location)
                    .and_then(|found| found.read_bytes())
            })? {
                Ok(bytes) => bytes,
                Err(err) => {
                    tracing::error!(location = %location, "cannot read a file for a zip: {err}");
                    failures.push((path, err.message().to_string()));
                    continue;
                }
            };
            let written = writer
                .start_file(path.as_str(), options)
                .map_err(|err| err.to_string())
                .and_then(|()| writer.write_all(&bytes).map_err(|err| err.to_string()));
            match written {
                Ok(()) => count += 1,
                Err(err) => return Err(Exception::throw_message(&ctx, &format!("writing the zip: {err}"))),
            }
        }
        let data = writer
            .finish()
            .map_err(|err| Exception::throw_message(&ctx, &format!("writing the zip: {err}")))?
            .into_inner();
        let size = i64::try_from(data.len()).unwrap_or(i64::MAX);
        with_resources(&ctx, |resources| resources.write_bytes(&target, &data))?;
        let mut map = indexmap::IndexMap::new();
        map.insert("count".to_string(), Value::I64(count));
        map.insert("size".to_string(), Value::I64(size));
        map.insert(
            "failures".to_string(),
            Value::List(
                failures
                    .into_iter()
                    .map(|(path, error)| {
                        let mut failure = indexmap::IndexMap::new();
                        failure.insert("path".to_string(), Value::string(path));
                        failure.insert("error".to_string(), Value::string(error));
                        Value::Map(failure)
                    })
                    .collect(),
            ),
        );
        host_to_js(&ctx, Value::Map(map))
    })
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

fn files_make_grant(ctx: Ctx<'_>, key: String, subject: String, ttl_seconds: f64) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        #[allow(clippy::cast_possible_truncation)]
        let ttl = if ttl_seconds.is_finite() {
            ttl_seconds as i64
        } else {
            3600
        };
        make_grant(&key, &subject, ttl).ok_or_else(|| Exception::throw_message(&ctx, "a grant needs a key"))
    })
}

fn files_check_grant(ctx: Ctx<'_>, key: String, subject: String, token: String) -> rquickjs::Result<bool> {
    host_guard(&ctx, || Ok(check_grant(&key, &subject, &token)))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[cfg(feature = "image")]
    #[test]
    fn a_thumbnail_fits_the_box_and_keeps_the_aspect() {
        let image = ::image::RgbImage::from_pixel(400, 200, ::image::Rgb([10, 120, 200]));
        let mut png = Cursor::new(Vec::new());
        ::image::DynamicImage::ImageRgb8(image)
            .write_to(&mut png, ::image::ImageFormat::Png)
            .unwrap();
        let (width, height, data) = thumbnail_png(&png.into_inner(), 100).unwrap();
        assert_eq!((width, height), (100, 50));
        assert!(data.starts_with(b"\x89PNG"));
        assert!(thumbnail_png(b"not an image", 100).is_none());
    }
}
