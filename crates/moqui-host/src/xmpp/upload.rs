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

//! `/xmpp-upload/<token>/<name>` (XEP-0363): an upload slot takes one PUT of the size it was asked for, then serves
//! the file at the same URL; a chat file sent to an XMPP client is served the same way (`XmppServices`). The token is
//! the capability, compared by its hash (`XmppUpload.tokenHash`). A file is served as an attachment, with no sniffing
//! and a sandbox, so a page uploaded here cannot run as this site; images, audio and video inline. CORS is open, as
//! web clients upload and fetch from their own origin.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_lib::object::{to_plain_string, Value};

use crate::http::HttpState;

pub(crate) const PREFIX: &str = "/xmpp-upload/";
const UPLOAD: &str = "moqui.xmpp.XmppUpload";

fn cors(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, HEAD, PUT, OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Content-Type"),
    );
    response
}

fn status(code: StatusCode, text: &'static str) -> Response {
    cors((code, text).into_response())
}

/// The upload row of a token.
fn row_of(factory: &ExecutionContextFactory, token: &str) -> Result<Option<moqui_core::entity::EntityValue>, String> {
    let hash = moqui_core::user::single_use_hash(token).map_err(|err| err.message().to_string())?;
    let facade = factory.entity().ok_or("no entity facade")?;
    let find = facade.find(UPLOAD).map_err(|err| err.to_string())?;
    find.eq("tokenHash", Value::string(hash))
        .one()
        .map_err(|err| err.to_string())
}

fn field(row: &moqui_core::entity::EntityValue, name: &str) -> String {
    row.get(name).map(to_plain_string).unwrap_or_default()
}

pub(crate) async fn handle(
    method: Method,
    Path((token, _name)): Path<(String, String)>,
    headers: HeaderMap,
    State(state): State<Arc<HttpState>>,
    body: Bytes,
) -> Response {
    if method == Method::OPTIONS {
        return status(StatusCode::NO_CONTENT, "");
    }
    let Some(server) = super::server_of(&state.factory) else {
        return status(StatusCode::NOT_FOUND, "not found");
    };
    if token.is_empty() || token.len() > 64 || !token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return status(StatusCode::NOT_FOUND, "not found");
    }
    let factory = Arc::downgrade(&state.factory);
    let max = server.config.upload_max_bytes;
    let put = method == Method::PUT;
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let done = crate::worker::on_pool(&factory, move |factory| {
        // the token is the capability: the file is read and written as internal work
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let row = match row_of(factory, &token) {
            Ok(Some(row)) => row,
            Ok(None) => return Ok(status(StatusCode::NOT_FOUND, "not found")),
            Err(err) => {
                tracing::warn!("XMPP upload lookup failed: {err}");
                return Ok(status(StatusCode::INTERNAL_SERVER_ERROR, "failed"));
            }
        };
        let location = field(&row, "location");
        if put {
            if !field(&row, "completedDate").is_empty() {
                return Ok(status(StatusCode::CONFLICT, "already uploaded"));
            }
            let size = field(&row, "fileSize").parse::<u64>().unwrap_or(0);
            if body.len() as u64 != size || size > max {
                return Ok(status(StatusCode::BAD_REQUEST, "the size is not the slot's"));
            }
            let declared = field(&row, "contentType");
            if !content_type.is_empty() && !declared.is_empty() && content_type != declared {
                return Ok(status(StatusCode::BAD_REQUEST, "the content type is not the slot's"));
            }
            let mut reader = std::io::Cursor::new(body.to_vec());
            if let Err(err) = factory.resource().write_from(&location, &mut reader, Some(size)) {
                tracing::warn!("XMPP upload {location} not stored: {}", err.message());
                return Ok(status(StatusCode::INTERNAL_SERVER_ERROR, "not stored"));
            }
            let facade = factory
                .entity()
                .ok_or_else(|| moqui_core::context::Error::new("no entity facade"))?;
            let mut done = facade.make_value(UPLOAD)?;
            done.set("xmppUploadId", Value::string(field(&row, "xmppUploadId")));
            done.set("completedDate", moqui_core::user::now_value());
            done.update(&facade)?;
            return Ok(status(StatusCode::CREATED, ""));
        }
        if field(&row, "completedDate").is_empty() {
            return Ok(status(StatusCode::NOT_FOUND, "not found"));
        }
        let bytes = match factory.resource().location_bytes(&location) {
            Ok(Some(bytes)) => bytes,
            _ => return Ok(status(StatusCode::NOT_FOUND, "not found")),
        };
        let kind = Some(field(&row, "contentType"))
            .filter(|kind| !kind.is_empty())
            .unwrap_or_else(|| "application/octet-stream".into());
        let inline = ["image/", "audio/", "video/"]
            .iter()
            .any(|prefix| kind.starts_with(prefix))
            && kind != "image/svg+xml";
        let name: String = field(&row, "fileName")
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || ".-_ ".contains(*ch))
            .collect();
        let mut response = (StatusCode::OK, bytes).into_response();
        let headers = response.headers_mut();
        if let Ok(value) = HeaderValue::from_str(&kind) {
            headers.insert(header::CONTENT_TYPE, value);
        }
        let disposition = format!(
            "{}; filename=\"{}\"",
            if inline { "inline" } else { "attachment" },
            name
        );
        if let Ok(value) = HeaderValue::from_str(&disposition) {
            headers.insert(header::CONTENT_DISPOSITION, value);
        }
        headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("sandbox; default-src 'none'"),
        );
        Ok(cors(response))
    })
    .await;
    done.unwrap_or_else(|err| {
        tracing::warn!("XMPP upload failed: {}", err.message());
        status(StatusCode::SERVICE_UNAVAILABLE, "unavailable")
    })
}
