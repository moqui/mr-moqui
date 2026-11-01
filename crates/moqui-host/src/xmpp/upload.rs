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
//! length and content type are the slot's. A second PUT of a slot while one is coming on this node is 409 at once. The
//! body is spooled as it arrives (under `runtime/tmp/upload`, so no request worker, database connection or memory is
//! held while a slow client sends), never more than the slot's size (a longer body is 413, a shorter one 400), within
//! `xmpp_upload_seconds` (300; past it 408). Then it is stored with the slot's row locked for update, in the
//! transaction that marks it done, so of PUTs on two nodes one writes and the other finds it done (409). A PUT that
//! fails leaves no file and the slot open.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::body::Body;
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
/// Bytes of a body gathered before they are written to its spool file.
const SPOOL_FEED: usize = 256 * 1024;
/// The longest the transaction that stores a spooled body may take.
const STORE_SECONDS: Duration = Duration::from_secs(120);

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
    /// It could not be spooled here.
    Failed,
}

impl Cut {
    fn response(self) -> Response {
        match self {
            Cut::TooLong => status(StatusCode::PAYLOAD_TOO_LARGE, "the body is over the slot's size"),
            Cut::TooShort | Cut::Broken => status(StatusCode::BAD_REQUEST, "the size is not the slot's"),
            Cut::Late => status(StatusCode::REQUEST_TIMEOUT, "the upload took too long"),
            Cut::Failed => status(StatusCode::INTERNAL_SERVER_ERROR, "not stored"),
        }
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
        // spooled with the web's uploads, under runtime/tmp/upload (swept at start)
        let spool_dir = state.factory.runtime_dir().join("tmp").join("upload");
        return put(
            &factory,
            spool_dir,
            token,
            &headers,
            body,
            server.config.upload_max_bytes,
        )
        .await;
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

/// A PUT to a slot: the token checked before the body is read, then the body spooled under the slot's size and the
/// PUT's deadline (no request worker or transaction held while it comes), then stored with the slot's row locked. See
/// the module notes.
async fn put(
    factory: &std::sync::Weak<ExecutionContextFactory>,
    spool_dir: PathBuf,
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

    // 2. one PUT of a slot at a time on this node: another while one is coming is 409 at once
    let Some(_claim) = Claim::take(&slot.id) else {
        return status(StatusCode::CONFLICT, "being uploaded");
    };

    // 3. the body, spooled as it comes, never past the slot's size, until the deadline
    let spool = match spool_body(&spool_dir, &slot, body, put_deadline()).await {
        Ok(spool) => spool,
        Err(cut) => return cut.response(),
    };

    // 4. stored, with the slot's row locked in the transaction that marks it done
    let written = crate::worker::on_pool(factory, move |factory| Ok(store(factory, &slot, &spool))).await;
    written.unwrap_or_else(|err| {
        tracing::warn!("XMPP upload failed: {}", err.message());
        status(StatusCode::SERVICE_UNAVAILABLE, "unavailable")
    })
}

/// The slots with a PUT coming on this node.
fn in_flight() -> &'static Mutex<HashSet<String>> {
    static IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()))
}

/// A slot's PUT in flight on this node, let go when dropped (so a PUT that failed can be tried again).
struct Claim(String);

impl Claim {
    fn take(slot_id: &str) -> Option<Self> {
        moqui_lib::sync::lock(in_flight())
            .insert(slot_id.to_string())
            .then(|| Self(slot_id.to_string()))
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        moqui_lib::sync::lock(in_flight()).remove(&self.0);
    }
}

/// A body spooled to a file of its own, removed when dropped.
struct Spooled(PathBuf);

impl Drop for Spooled {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Spool a slot's body under `dir` as it comes: never more than the slot's size, the whole of it, within `deadline`.
/// The file is written on blocking threads a batch at a time; no request worker waits on the client.
async fn spool_body(dir: &std::path::Path, slot: &Slot, body: Body, deadline: Duration) -> Result<Spooled, Cut> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name: String = slot
        .id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect();
    let path = dir.join(format!(
        "xmpp-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let spool = Spooled(path.clone());
    let failed = |err: &dyn std::fmt::Display| {
        tracing::warn!("XMPP upload {}: not spooled: {err}", slot.location);
        Cut::Failed
    };
    let file = {
        let (dir, path) = (dir.to_path_buf(), path.clone());
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir)?;
            std::fs::File::create(&path)
        })
        .await
        .map_err(|err| failed(&err))?
        .map_err(|err| failed(&err))?
    };
    let file = Arc::new(file);
    let size = slot.size;
    let read = async {
        let mut stream = body.into_data_stream();
        let mut pending: Vec<u8> = Vec::new();
        let mut total = 0_u64;
        loop {
            let done = match stream.next().await {
                Some(Ok(chunk)) => {
                    total = total.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
                    if total > size {
                        return Err(Cut::TooLong);
                    }
                    pending.extend_from_slice(&chunk);
                    false
                }
                Some(Err(_)) => return Err(Cut::Broken),
                None => true,
            };
            if pending.len() >= SPOOL_FEED || (done && !pending.is_empty()) {
                let batch = std::mem::take(&mut pending);
                let file = Arc::clone(&file);
                tokio::task::spawn_blocking(move || (&*file).write_all(&batch))
                    .await
                    .map_err(|err| failed(&err))?
                    .map_err(|err| failed(&err))?;
            }
            if done {
                return if total < size { Err(Cut::TooShort) } else { Ok(()) };
            }
        }
    };
    match tokio::time::timeout(deadline, read).await {
        Ok(Ok(())) => Ok(spool),
        Ok(Err(cut)) => Err(cut),
        Err(_) => Err(Cut::Late),
    }
}

/// Store a spooled body with the slot's row locked, in the transaction that marks it done: of PUTs on two nodes one
/// writes and the other finds it done (409). A body that does not go in whole leaves no file and the slot open. The
/// answer: 201, or why not.
fn store(factory: &ExecutionContextFactory, slot: &Slot, spool: &Spooled) -> Response {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let Some(facade) = factory.entity() else {
        return status(StatusCode::SERVICE_UNAVAILABLE, "unavailable");
    };
    let mut refused: Option<Response> = None;
    let mut wrote = false;
    let outcome = facade.transactions_for(UPLOAD).and_then(|tx| {
        tx.run(
            moqui_core::transaction::TransactionAttribute::ForceNew,
            Some(STORE_SECONDS),
            || {
                // the lock: a PUT of the same slot on another node waits here, then finds the slot done
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
                let mut input = std::fs::File::open(&spool.0).map_err(|err| {
                    moqui_lib::error::Error::new(format!("the spooled upload could not be read: {err}"))
                })?;
                wrote = true;
                factory
                    .resource()
                    .write_from(&slot.location, &mut input, Some(slot.size))
                    .map_err(|err| moqui_lib::error::Error::new(err.message().to_string()))?;
                let mut done = facade.make_value(UPLOAD)?;
                done.set("xmppUploadId", Value::string(&slot.id));
                done.set("completedDate", moqui_core::user::now_value());
                done.update(&facade)?;
                Ok(())
            },
        )
    });
    if let Some(refused) = refused {
        return refused;
    }
    if let Err(err) = outcome {
        tracing::warn!("XMPP upload {} not stored: {err}", slot.location);
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
        return status(StatusCode::INTERNAL_SERVER_ERROR, "not stored");
    }
    status(StatusCode::CREATED, "")
}
