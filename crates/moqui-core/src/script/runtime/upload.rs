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

//! An uploaded file and a stream as QuickJS scripts see them: objects of the host table's classes
//! (`script/host/upload.rs`).

use std::sync::Arc;

use super::*;
use crate::script::host::upload::{STREAM, UPLOAD};
use crate::web::multipart::UploadedFile;

pub(crate) use crate::script::host::StreamBytes;

/// An uploaded file as a script value.
pub(super) fn upload_to_js<'js>(ctx: &Ctx<'js>, inner: Arc<UploadedFile>) -> rquickjs::Result<JsValue<'js>> {
    bind::running_instance(ctx, HostObject(inner as _), &UPLOAD).map(Object::into_value)
}

/// A stream as a script value.
pub(super) fn stream_to_js<'js>(ctx: &Ctx<'js>, inner: Arc<StreamBytes>) -> rquickjs::Result<JsValue<'js>> {
    bind::running_instance(ctx, HostObject(inner as _), &STREAM).map(Object::into_value)
}

/// The uploaded file or stream an object of the table's classes holds, as itself: passed to a service it stays the
/// file, as the Java FileItem in `ec.web.parameters` does, and a stream stays the stream (it has no own properties,
/// so it would be an empty map, which a write would store as the text `{}`).
pub(super) fn upload_or_stream(object: &HostObject) -> Option<Value> {
    (object.downcast::<UploadedFile>().is_some() || object.downcast::<StreamBytes>().is_some())
        .then(|| Value::Object(object.clone()))
}


/// An uploaded file made from text, as the JavaScript object a service or spec holds. The test harness uses it to call
/// the services that take a `FileItem` without a web request.
pub fn text_upload_to_js<'js>(
    ctx: &Ctx<'js>,
    field_name: &str,
    file_name: &str,
    content_type: &str,
    text: &str,
) -> rquickjs::Result<JsValue<'js>> {
    let inner = Arc::new(UploadedFile::in_memory(
        field_name,
        file_name,
        content_type,
        text.as_bytes().to_vec(),
    ));
    upload_to_js(ctx, inner)
}
