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

//! An uploaded file and a stream of bytes as a script sees them ([`UPLOAD`], [`STREAM`]): what the Java `FileItem`
//! showed (name, type, size) and the ways to keep the bytes. A script cannot hold a Java stream, so the bytes go to a
//! resource location (streamed, so a large upload is not read into memory), or are read as text, as a `Uint8Array`, or
//! a window of them at a time. Each engine makes an object of the class from a host value that holds an
//! [`UploadedFile`] or a [`StreamBytes`], and gives that value back when the object is passed on.

use std::sync::Arc;

use moqui_lib::object::{HostObject, Value};

use super::{decode_text, method, with_ec, Call, HostClass, HostError, Ret, StreamBytes};
use crate::web::multipart::UploadedFile;

/// An uploaded file (`ec.web.parameters`' file items, the test harness's text uploads).
pub(crate) static UPLOAD: HostClass = HostClass {
    name: "host:UploadedFile",
    methods: &[
        method!("UploadedFile", "getName", [], None, leaf, upload_name),
        method!("UploadedFile", "getFieldName", [], None, leaf, upload_field_name),
        method!("UploadedFile", "getContentType", [], None, leaf, upload_content_type),
        method!("UploadedFile", "getSize", [], None, leaf, upload_size),
        method!("UploadedFile", "isInMemory", [], None, leaf, upload_in_memory),
        method!("UploadedFile", "getString", [OptValue], None, leaf, upload_string),
        method!("UploadedFile", "getInputStream", [], None, leaf, upload_stream),
        // a location may be in the database (`dbresource://`)
        method!("UploadedFile", "write", [Text], upload_write),
        method!("UploadedFile", "toString", [], None, leaf, upload_name),
    ],
    getters: &[
        method!("UploadedFile", "name", [], None, leaf, upload_name),
        method!("UploadedFile", "fileName", [], None, leaf, upload_name),
        method!("UploadedFile", "fieldName", [], None, leaf, upload_field_name),
        method!("UploadedFile", "contentType", [], None, leaf, upload_content_type),
        method!("UploadedFile", "size", [], None, leaf, upload_size),
    ],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

/// The bytes of an uploaded file or a resource, as the Java `InputStream` a script passes to `putStream`. A script
/// cannot read it a byte at a time: it reads all of it as text or a `Uint8Array`, a window of it from an offset
/// (`read(offset, max)`, for a byte range), or writes it to a location, which takes it as it is read. Its source may be
/// a database blob, so a read is not a leaf.
pub(crate) static STREAM: HostClass = HostClass {
    name: "host:InputStream",
    methods: &[
        method!("InputStream", "close", [], None, leaf, stream_close),
        method!("InputStream", "read", [Number, Number], stream_read),
        method!("InputStream", "available", [], None, leaf, stream_size),
        method!("InputStream", "readAllBytes", [], stream_all_bytes),
        method!("InputStream", "getText", [OptValue], stream_text),
        method!("InputStream", "write", [Text], stream_write),
    ],
    getters: &[method!("InputStream", "size", [], None, leaf, stream_size)],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

/// An uploaded file holding `text`, as a method returns it: the test harness's `uploadedFile`, for the services that
/// take a `FileItem` without a web request.
pub fn text_upload(field_name: &str, file_name: &str, content_type: &str, text: &str) -> Ret {
    let file = UploadedFile::in_memory(field_name, file_name, content_type, text.as_bytes().to_vec());
    Ret::Object(HostObject::new(file), &UPLOAD)
}

/// The most bytes one `read(offset, max)` returns, so a script reading a large file a window at a time never holds
/// more than this of it.
pub(crate) const MAX_STREAM_READ: usize = 8 * 1024 * 1024;

fn message(err: impl std::fmt::Display) -> HostError {
    HostError::message(err.to_string())
}

/// A charset name given to a script call: a non-empty string, else none.
fn charset_arg(call: &Call<'_>, index: usize) -> Option<String> {
    match call.value(index) {
        Some(Value::String(text)) => Some(text.trim().to_string()).filter(|text| !text.is_empty()),
        _ => None,
    }
}

fn size_value(size: u64) -> Ret {
    Ret::Value(Value::I64(i64::try_from(size).unwrap_or(i64::MAX)))
}

fn this_upload(call: &Call<'_>) -> Result<Arc<UploadedFile>, HostError> {
    call.this::<UploadedFile>()
}

fn upload_name(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(this_upload(call)?.file_name.clone()))
}

fn upload_field_name(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(this_upload(call)?.field_name.clone()))
}

fn upload_content_type(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(this_upload(call)?.content_type.clone()))
}

fn upload_size(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(size_value(this_upload(call)?.size()))
}

fn upload_in_memory(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(this_upload(call)?.is_in_memory()))
}

/// The bytes as text in `charset` (Java's `FileItem.getString(encoding)`), UTF-8 without one.
fn upload_string(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let bytes = this_upload(call)?.read_bytes().map_err(|err| message(err.message()))?;
    let text = decode_text(&bytes, charset_arg(call, 0).as_deref()).map_err(|err| message(err.message()))?;
    Ok(Ret::from(text))
}

/// A stream over `source`, as a host value.
pub(crate) fn stream_value(source: Arc<dyn crate::resource::source::ByteSource>) -> Value {
    Value::Object(HostObject::new(StreamBytes { source }))
}

/// The bytes as a stream, as the Java `FileItem.getInputStream`. A script hands it to `putStream` of a resource
/// reference and closes it.
fn upload_stream(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let source = this_upload(call)?.source().map_err(|err| message(err.message()))?;
    Ok(Ret::Value(stream_value(source)))
}

/// Store the file at a resource location, as it is read.
fn upload_write(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let source = this_upload(call)?.source().map_err(|err| message(err.message()))?;
    put(&StreamBytes { source }, &call.text_or_empty(0))?;
    Ok(Ret::Undefined)
}

fn this_stream(call: &Call<'_>) -> Result<Arc<StreamBytes>, HostError> {
    call.this::<StreamBytes>()
}

/// Write the content to `location` a window at a time.
fn put(stream: &StreamBytes, location: &str) -> Result<(), HostError> {
    let mut reader = crate::resource::source::SourceReader::new(Arc::clone(&stream.source));
    let size = stream.source.len();
    with_ec(|ec| ec.factory().resource().write_from(location, &mut reader, Some(size)))?
        .map_err(|err| message(err.message()))
}

/// Nothing is held open, so closing is only for the script's `try`/`finally`.
fn stream_close(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::Undefined)
}

/// Up to `max` bytes from `offset` as a `Uint8Array`, at most [`MAX_STREAM_READ`]: fewer only at the end or at that
/// cap, none past the end. Only the window is read from the source (a file, a database blob), so a byte range of a
/// large file does not read the rest (Shop F88).
fn stream_read(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let stream = this_stream(call)?;
    let offset = call.number(0).unwrap_or(f64::NAN);
    let max = call.number(1).unwrap_or(f64::NAN);
    let whole = |value: f64| value.is_finite() && value >= 0.0 && value.fract() == 0.0;
    if !whole(offset) || !whole(max) {
        return Err(HostError::message(format!(
            "read(offset, max) takes whole numbers from 0, not {offset} and {max}"
        )));
    }
    // whole, non-negative and finite, so these casts only saturate at the type's top
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (offset, max) = (offset as u64, (max as u64).min(MAX_STREAM_READ as u64) as usize);
    let bytes = if max == 0 || offset >= stream.source.len() {
        Vec::new()
    } else {
        stream
            .source
            .read_at(offset, max)
            .map_err(|err| message(err.message()))?
    };
    Ok(Ret::Value(Value::Bytes(bytes)))
}

/// The number of bytes, as an uploaded file's `size`, for a quota check.
fn stream_size(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(size_value(this_stream(call)?.source.len()))
}

fn stream_all_bytes(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let bytes = this_stream(call)?.bytes().map_err(|err| message(err.message()))?;
    Ok(Ret::Value(Value::Bytes(bytes)))
}

/// The bytes as text in `charset`, UTF-8 without one.
fn stream_text(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let bytes = this_stream(call)?.bytes().map_err(|err| message(err.message()))?;
    let text = decode_text(&bytes, charset_arg(call, 0).as_deref()).map_err(|err| message(err.message()))?;
    Ok(Ret::from(text))
}

/// Store the bytes at a location, as `putStream` of that location's reference does.
fn stream_write(call: &mut Call<'_>) -> Result<Ret, HostError> {
    put(&*this_stream(call)?, &call.text_or_empty(0))?;
    Ok(Ret::Undefined)
}
