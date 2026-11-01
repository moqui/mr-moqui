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

//! `/xmpp-upload/<token>/<name>` (XEP-0363): an upload slot takes one PUT of the size it was asked for, within an hour,
//! then serves the file at the same URL; a chat file sent to an XMPP client is served the same way (`XmppServices`). The token is
//! the capability, compared by its hash (`XmppUpload.tokenHash`). A file is served as an attachment, with no sniffing
//! and a sandbox, so a page uploaded here cannot run as this site; images, audio and video inline. CORS is open, as
//! web clients upload and fetch from their own origin.
//!
//! A PUT is checked before its body is read: the token names an open slot (not put yet, within its hour), and a declared
//! length and content type are the slot's. Then the body is streamed to storage, never more than the slot's size (a
//! longer body is 413, a shorter one 400), within `xmpp_upload_seconds` (300; past it 408). The slot's row is locked
//! for update while the body goes in, in the transaction that marks it done, so of two PUTs at once one writes and the
//! other, once the lock is let go, finds it done (409). A PUT that fails leaves no file and the slot open.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_lib::object::{to_plain_string, Value};

use crate::http::HttpState;

pub(crate) const PREFIX: &str = "/xmpp-upload/";
const UPLOAD: &str = "moqui.xmpp.XmppUpload";
/// How long an upload slot takes its PUT.
const SLOT_LIFETIME_MS: i64 = 3_600_000;
/// The longest a PUT may take, by default (`xmpp_upload_seconds`).
const DEFAULT_PUT_SECONDS: u64 = 300;
/// Body chunks waiting for the writer.
const CHUNKS_WAITING: usize = 8;

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

/// The longest a PUT may take (`xmpp_upload_seconds`).
fn put_deadline() -> Duration {
    let seconds = moqui_lib::env::get_prop_or_env("xmpp_upload_seconds")
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_PUT_SECONDS);
    Duration::from_secs(seconds)
}

/// Why a body did not go in whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cut {
    /// More than the slot's size.
    TooLong,
    /// The body ended before the slot's size.
    TooShort,
    /// The client broke off.
    Broken,
    /// Past the PUT's deadline.
    Late,
}

impl Cut {
    fn response(self) -> Response {
        match self {
            Cut::TooLong => status(StatusCode::PAYLOAD_TOO_LARGE, "the body is over the slot's size"),
            Cut::TooShort | Cut::Broken => status(StatusCode::BAD_REQUEST, "the size is not the slot's"),
            Cut::Late => status(StatusCode::REQUEST_TIMEOUT, "the upload took too long"),
        }
    }
}

/// The body as the storage writer reads it: chunks from the request, at most the slot's size, ending exactly there.
struct SlotBody {
    chunks: tokio::sync::mpsc::Receiver<Result<Bytes, Cut>>,
    current: Bytes,
    left: u64,
    cut: Arc<Mutex<Option<Cut>>>,
}

impl SlotBody {
    fn fail(&self, cut: Cut) -> std::io::Error {
        *moqui_lib::sync::lock(&self.cut) = Some(cut);
        std::io::Error::other(format!("{cut:?}"))
    }
}

impl std::io::Read for SlotBody {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.current.is_empty() {
            match self.chunks.blocking_recv() {
                Some(Ok(chunk)) => {
                    let length = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
                    if length > self.left {
                        return Err(self.fail(Cut::TooLong));
                    }
                    self.left -= length;
                    self.current = chunk;
                }
                Some(Err(cut)) => return Err(self.fail(cut)),
                None if self.left > 0 => return Err(self.fail(Cut::TooShort)),
                None => return Ok(0),
            }
        }
        let taken = out.len().min(self.current.len());
        out[..taken].copy_from_slice(&self.current.split_to(taken));
        Ok(taken)
    }
}

/// An open slot, checked before its body is read.
struct Slot {
    id: String,
    location: String,
    size: u64,
}

pub(crate) async fn handle(
    method: Method,
    Path((token, _name)): Path<(String, String)>,
    headers: HeaderMap,
    State(state): State<Arc<HttpState>>,
    body: Body,
) -> Response {
    if method == Method::OPTIONS {
        return status(StatusCode::NO_CONTENT, "");
    }
    let Some(server) = super::server_of(&state.factory) else {
        return status(StatusCode::NOT_FOUND, "not found");
    };
    if token.is_empty()
        || token.len() > 64
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return status(StatusCode::NOT_FOUND, "not found");
    }
    let factory = Arc::downgrade(&state.factory);
    if method == Method::PUT {
        return put(&factory, token, &headers, body, server.config.upload_max_bytes).await;
    }
    drop(body);
    let done = crate::worker::on_pool(&factory, move |factory| {
        // the token is the capability: the file is read as internal work
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

/// A PUT to a slot: the token checked before the body is read, then the body streamed to storage under the slot's
/// size and the PUT's deadline, with the slot's row locked. See the module notes.
async fn put(
    factory: &std::sync::Weak<ExecutionContextFactory>,
    token: String,
    headers: &HeaderMap,
    body: Body,
    max: u64,
) -> Response {
    let header_text = |name: header::HeaderName| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let content_type = header_text(header::CONTENT_TYPE);
    let declared = header_text(header::CONTENT_LENGTH).parse::<u64>().ok();
    // 1. the token, before any of the body is read
    let checked = crate::worker::on_pool(factory, move |factory| {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let row = match row_of(factory, &token) {
            Ok(Some(row)) => row,
            Ok(None) => return Ok(Err(status(StatusCode::NOT_FOUND, "not found"))),
            Err(err) => {
                tracing::warn!("XMPP upload lookup failed: {err}");
                return Ok(Err(status(StatusCode::INTERNAL_SERVER_ERROR, "failed")));
            }
        };
        if !field(&row, "completedDate").is_empty() {
            return Ok(Err(status(StatusCode::CONFLICT, "already uploaded")));
        }
        // a slot is good for an hour (clean#Uploads removes it later)
        let made = row.get("createdDate").and_then(moqui_core::user::value_millis);
        if made.is_none_or(|made| moqui_core::user::now_millis() - made > SLOT_LIFETIME_MS) {
            return Ok(Err(status(StatusCode::NOT_FOUND, "not found")));
        }
        let size = field(&row, "fileSize").parse::<u64>().unwrap_or(0);
        if size > max || declared.is_some_and(|declared| declared != size) {
            return Ok(Err(status(StatusCode::BAD_REQUEST, "the size is not the slot's")));
        }
        let kind = field(&row, "contentType");
        if !content_type.is_empty() && !kind.is_empty() && content_type != kind {
            return Ok(Err(status(
                StatusCode::BAD_REQUEST,
                "the content type is not the slot's",
            )));
        }
        Ok(Ok(Slot {
            id: field(&row, "xmppUploadId"),
            location: field(&row, "location"),
            size,
        }))
    })
    .await;
    let slot = match checked {
        Ok(Ok(slot)) => slot,
        Ok(Err(refused)) => return refused,
        Err(err) => {
            tracing::warn!("XMPP upload failed: {}", err.message());
            return status(StatusCode::SERVICE_UNAVAILABLE, "unavailable");
        }
    };

    // 2. the body, chunk by chunk to the writer, until the deadline
    let deadline = put_deadline();
    let (sender, chunks) = tokio::sync::mpsc::channel::<Result<Bytes, Cut>>(CHUNKS_WAITING);
    let cut = Arc::new(Mutex::new(None));
    let reader = SlotBody {
        chunks,
        current: Bytes::new(),
        left: slot.size,
        cut: Arc::clone(&cut),
    };
    let forward = {
        let sender = sender.clone();
        async move {
            let mut stream = body.into_data_stream();
            while let Some(chunk) = stream.next().await {
                let item = chunk.map_err(|_| Cut::Broken);
                let stop = item.is_err();
                if sender.send(item).await.is_err() || stop {
                    break;
                }
            }
        }
    };
    let forward = async move {
        if tokio::time::timeout(deadline, forward).await.is_err() {
            let _ = sender.send(Err(Cut::Late)).await;
        }
    };
    let write = crate::worker::on_pool(factory, move |factory| Ok(store(factory, &slot, reader, deadline)));
    let ((), written) = tokio::join!(forward, write);
    match written {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!("XMPP upload failed: {}", err.message());
            match *moqui_lib::sync::lock(&cut) {
                Some(cut) => cut.response(),
                None => status(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
            }
        }
    }
}

/// Write a slot's body with its row locked, in the transaction that marks it done; a body that does not go in whole
/// leaves no file and the slot open. The answer: 201, or why not.
fn store(factory: &ExecutionContextFactory, slot: &Slot, mut reader: SlotBody, deadline: Duration) -> Response {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let Some(facade) = factory.entity() else {
        return status(StatusCode::SERVICE_UNAVAILABLE, "unavailable");
    };
    let cut = Arc::clone(&reader.cut);
    let mut refused: Option<Response> = None;
    let mut wrote = false;
    let outcome = facade.transactions_for(UPLOAD).and_then(|tx| {
        tx.run(
            moqui_core::transaction::TransactionAttribute::ForceNew,
            Some(deadline + Duration::from_secs(60)),
            || {
                // the lock: a second PUT waits here, then finds the slot done
                let row = facade
                    .find(UPLOAD)?
                    .eq("xmppUploadId", Value::string(&slot.id))
                    .for_update(true)
                    .one()?;
                let Some(row) = row else {
                    refused = Some(status(StatusCode::NOT_FOUND, "not found"));
                    return Ok(());
                };
                if !field(&row, "completedDate").is_empty() {
                    refused = Some(status(StatusCode::CONFLICT, "already uploaded"));
                    return Ok(());
                }
                wrote = true;
                if let Err(err) = factory
                    .resource()
                    .write_from(&slot.location, &mut reader, Some(slot.size))
                {
                    let cut = *moqui_lib::sync::lock(&cut);
                    if cut.is_none() {
                        tracing::warn!("XMPP upload {} not stored: {}", slot.location, err.message());
                    }
                    refused = Some(cut.map_or_else(
                        || status(StatusCode::INTERNAL_SERVER_ERROR, "not stored"),
                        Cut::response,
                    ));
                    return Err(moqui_lib::error::Error::new("the upload did not go in whole"));
                }
                let mut done = facade.make_value(UPLOAD)?;
                done.set("xmppUploadId", Value::string(&slot.id));
                done.set("completedDate", moqui_core::user::now_value());
                done.update(&facade)?;
                Ok(())
            },
        )
    });
    if let Some(refused) = refused {
        if wrote {
            // a file cut off goes (a database resource went with the rollback)
            if let Err(err) = factory.resource().delete(&slot.location) {
                tracing::debug!(
                    "XMPP upload {}: the partial file was not removed: {}",
                    slot.location,
                    err.message()
                );
            }
        }
        return refused;
    }
    if let Err(err) = outcome {
        tracing::warn!("XMPP upload {} not kept: {err}", slot.location);
        if wrote {
            let _ = factory.resource().delete(&slot.location);
        }
        return status(StatusCode::INTERNAL_SERVER_ERROR, "not stored");
    }
    status(StatusCode::CREATED, "")
}
