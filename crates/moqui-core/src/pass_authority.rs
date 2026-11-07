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

//! The PASS authority's operations over gRPC (plans/Edge-Node.md §5.5), the part that needs no network:
//! - on the authority, [`serve`] and [`serve_download`] run one call of the host's `PassAuthority` service: they check
//!   the client, bind the authority session an account flow carries as its handle, and run the authority service
//!   (`pass_authority_service`, Mantle's `mantle.pass.PassAuthorityServices.handle#Call` by default), the same function
//!   Mantle's REST servlet calls after its own checks;
//! - on a PASS frontend, the [`PassSender`] the host installs carries scripts' calls to an authority
//!   (`ec.service.passAuthority(endpoint)`): a direct gRPC endpoint, or the edge link. With none installed (Lite and Dev
//!   have no gRPC) a call fails with [`NOT_BUILT`], and mr-pass's automatic transport takes REST (decision 8).
//!
//! The input and the result are JSON (decision 7). The client is a registered PASS client: by its secret on the direct
//! listener, checked by the framework's own check (`org.moqui.public.AccountServices.check#Client`, the one the REST
//! servlet uses), or the edge node on its link, by the id `pass_edge_client` (default `edge`) and with no secret. The
//! visitor's address is data for rate limits only.

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Arc;

use moqui_lib::error::Error;
use moqui_lib::json::{from_json, to_json};
use moqui_lib::object::Value;
use moqui_lib::sync::InstallSlot;

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::resource::source::ByteSource;

/// What a script's call to a gRPC authority fails with when the host has no gRPC client (Lite, Dev).
pub const NOT_BUILT: &str = "gRPC not built into this host";
/// The property naming the service that runs an operation on the authority.
pub const SERVICE_PROPERTY: &str = "pass_authority_service";
/// The authority service when [`SERVICE_PROPERTY`] is empty: Mantle's.
pub const DEFAULT_SERVICE: &str = "mantle.pass.PassAuthorityServices.handle#Call";
/// The property naming the PASS client the edge node is on its link (`pass_client_<id>_sites` lists its sites).
pub const EDGE_CLIENT_PROPERTY: &str = "pass_edge_client";
/// The edge node's client id when [`EDGE_CLIENT_PROPERTY`] is empty.
pub const DEFAULT_EDGE_CLIENT: &str = "edge";
/// The largest file an upload carries, as the REST API's multipart limit.
pub const MAX_UPLOAD_BYTES: u64 = 10 * 1024 * 1024;
/// The prefix of the account actions' operation names: `account.session`, `account.challenge`, ...
pub const ACCOUNT_PREFIX: &str = "account.";

/// The framework's check of a registered client's secret.
const CHECK_CLIENT: &str = "org.moqui.public.AccountServices.check#Client";

/// One operation for the authority.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Call {
    pub operation: String,
    pub site_id: String,
    /// A JSON object.
    pub input: serde_json::Value,
    /// The visitor's delegated credential, empty when not signed in.
    pub delegation: String,
    /// The visitor's address, for rate limits only.
    pub visitor_address: String,
    pub operation_id: String,
    /// The authority session of an account flow in progress.
    pub session_handle: String,
    /// The registered client, on the direct listener; the sender fills it from `pass_client_id`.
    pub client_id: String,
}

/// One field's error in a [`PassError`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

/// The REST API's error envelope, with the HTTP status the REST API answers it with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PassError {
    pub code: String,
    pub message: String,
    pub field_errors: Vec<FieldError>,
    pub correlation_id: String,
    pub status: u16,
}

impl PassError {
    /// The authority could not be reached or did not answer: the REST API's 503.
    pub fn unavailable(message: &str) -> Self {
        Self {
            code: "Unavailable".to_string(),
            message: message.to_string(),
            field_errors: Vec::new(),
            correlation_id: String::new(),
            status: 503,
        }
    }

    /// The envelope as the REST API writes it, with its status: `{code, message, fieldErrors, correlationId, status}`.
    pub fn to_json(&self) -> serde_json::Value {
        let fields: Vec<serde_json::Value> = self
            .field_errors
            .iter()
            .map(|error| serde_json::json!({"field": error.field, "message": error.message}))
            .collect();
        serde_json::json!({
            "code": self.code,
            "message": self.message,
            "fieldErrors": fields,
            "correlationId": self.correlation_id,
            "status": self.status,
        })
    }

    /// An envelope from its JSON; a missing status is the one of its code, as the REST API maps them.
    pub fn from_json(json: &serde_json::Value) -> Self {
        let text = |name: &str| {
            json.get(name)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let code = match text("code") {
            code if code.is_empty() => "Unavailable".to_string(),
            code => code,
        };
        let status = json
            .get("status")
            .and_then(serde_json::Value::as_u64)
            .and_then(|status| u16::try_from(status).ok())
            .filter(|status| (400..600).contains(status))
            .unwrap_or_else(|| status_of(&code));
        let field_errors = json
            .get("fieldErrors")
            .and_then(serde_json::Value::as_array)
            .map(|list| {
                list.iter()
                    .map(|error| FieldError {
                        field: error["field"].as_str().unwrap_or_default().to_string(),
                        message: error["message"].as_str().unwrap_or_default().to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            code,
            message: text("message"),
            field_errors,
            correlation_id: text("correlationId"),
            status,
        }
    }
}

/// The REST API's status for an error kind (Mantle's PassApi.js).
pub fn status_of(code: &str) -> u16 {
    match code {
        "AuthenticationRequired" => 401,
        "Unauthorized" => 403,
        "NotFound" => 404,
        "MethodNotAllowed" => 405,
        "Conflict" => 409,
        "Unprocessable" => 422,
        "Tarpit" => 429,
        _ => 503,
    }
}

/// The authority's answer to a call.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    /// The result, a JSON object, or the error envelope.
    pub result: Result<serde_json::Value, PassError>,
    /// The authority session after an account action, empty for any other call.
    pub session_handle: String,
}

impl Reply {
    /// An answer that is only the error envelope.
    pub fn error(error: PassError) -> Self {
        Self {
            result: Err(error),
            session_handle: String::new(),
        }
    }
}

/// A file for an operation that takes one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UploadFile {
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// A `Cache-Control` a download may carry instead of `no-store`: a public product image of the current generation
/// (Shop P8, F75 and F118). The authority names it; any other value is sent as `no-store` ([`download_cache_control`]).
pub const PUBLIC_IMAGE_CACHE: &str = "public, max-age=60";

/// The other `Cache-Control` a download may carry: a theme release's binary asset, read by its digest, whose bytes never
/// change (Shop F55). Only [`THEME_ASSET_OPERATION`] may carry it.
pub const THEME_ASSET_CACHE: &str = "public, max-age=31536000, immutable";

/// The download operation of a theme release's binary asset by digest, the only one that may carry [`THEME_ASSET_CACHE`].
pub const THEME_ASSET_OPERATION: &str = "getShopThemeAsset";

/// The `Cache-Control` a download of `operation` answers with in place of `no-store`, given the value the authority named:
/// exact values only, [`PUBLIC_IMAGE_CACHE`] for any download and [`THEME_ASSET_CACHE`] for a theme asset; anything else
/// is none (`no-store`).
pub fn download_cache_control(operation: &str, value: &str) -> Option<&'static str> {
    if value == PUBLIC_IMAGE_CACHE {
        Some(PUBLIC_IMAGE_CACHE)
    } else if value == THEME_ASSET_CACHE && operation == THEME_ASSET_OPERATION {
        Some(THEME_ASSET_CACHE)
    } else {
        None
    }
}

/// What a downloaded file is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DownloadMeta {
    pub content_type: String,
    pub filename: String,
    /// The bytes sent: the file's length, the range's, or 0 for a range outside the file; none when not known.
    pub length: Option<u64>,
    /// `attachment` or `inline`.
    pub disposition: String,
    /// For a file that takes byte ranges (a granted Shop download, F88): what of it is sent.
    pub ranged: Option<Ranged>,
    /// [`PUBLIC_IMAGE_CACHE`] or [`THEME_ASSET_CACHE`] when the authority allows it ([`download_cache_control`]), else
    /// none (`no-store`).
    pub cache_control: Option<String>,
}

/// What of a file that takes byte ranges a download sends, as a static file's `Range` answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ranged {
    /// The whole file (200, with `Accept-Ranges`).
    Whole,
    /// Bytes `start` to `end`, both included, of a file of `complete` bytes (206).
    Part { start: u64, end: u64, complete: u64 },
    /// A range outside a file of `complete` bytes (416, no body).
    Unsatisfiable { complete: u64 },
}

impl Ranged {
    /// The HTTP status of the answer.
    pub fn status(self) -> u16 {
        match self {
            Self::Whole => 200,
            Self::Part { .. } => 206,
            Self::Unsatisfiable { .. } => 416,
        }
    }

    /// The `Content-Range` header's value, for a part or a range outside the file.
    pub fn content_range(self) -> Option<String> {
        match self {
            Self::Whole => None,
            Self::Part { start, end, complete } => Some(format!("bytes {start}-{end}/{complete}")),
            Self::Unsatisfiable { complete } => Some(format!("bytes */{complete}")),
        }
    }

    /// The range of a file of `complete` bytes as the authority answered it (`{start, end}`, `{unsatisfiable}` or
    /// null for the whole file), checked again against the file: a start past its end or an end before the start is
    /// outside it, and an end past it is cut to its last byte.
    pub fn of(range: &serde_json::Value, complete: u64) -> Self {
        if range.is_null() {
            return Self::Whole;
        }
        let bound = |name: &str| range.get(name).and_then(serde_json::Value::as_u64);
        let refused = range.get("unsatisfiable").and_then(serde_json::Value::as_bool) == Some(true);
        match (bound("start"), bound("end")) {
            (Some(start), Some(end)) if !refused && start <= end && start < complete => Self::Part {
                start,
                end: end.min(complete - 1),
                complete,
            },
            _ => Self::Unsatisfiable { complete },
        }
    }
}

/// A downloaded file as it arrives.
pub struct Download {
    pub meta: DownloadMeta,
    pub reader: Box<dyn Read + Send>,
}

/// A file the authority serves, read a window at a time: `meta.length` bytes of `source` from `start`.
pub struct Served {
    pub meta: DownloadMeta,
    pub source: Arc<dyn ByteSource>,
    /// The offset of the first byte sent: 0, or a range's start.
    pub start: u64,
}

/// Sends calls to a PASS authority on the host's gRPC: `endpoint` is `grpc://host:port` or `grpcs://host:port` (the
/// authority's gRPC listener, the client named by `pass_client_id` with `pass_client_secret`), or `edge:` (on an edge
/// node, its local node down the link). A transport failure is an error; the authority's refusal is a [`PassError`].
pub trait PassSender: Send + Sync {
    fn call(&self, endpoint: &str, call: Call) -> Result<Reply, Error>;
    fn download(&self, endpoint: &str, call: Call) -> Result<Result<Download, PassError>, Error>;
    fn upload(&self, endpoint: &str, call: Call, file: UploadFile) -> Result<Reply, Error>;
}

static SENDER: InstallSlot<dyn PassSender> = InstallSlot::new();

/// Use `sender` for scripts' calls to a gRPC authority in this process.
pub fn install_pass_sender(sender: Arc<dyn PassSender>) {
    SENDER.install(sender);
}

/// Remove the installed sender.
pub fn clear_pass_sender() {
    SENDER.clear();
}

/// The installed sender, or [`NOT_BUILT`].
///
/// # Errors
///
/// Returns [`NOT_BUILT`] when the host installed none.
pub fn sender() -> Result<Arc<dyn PassSender>, Error> {
    SENDER.get().ok_or_else(|| Error::new(NOT_BUILT))
}

/// The client of a call to the authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Client {
    /// A registered client on the direct listener, with the secret it presented, checked by [`serve`].
    Secret { id: String, secret: String },
    /// A registered client on the direct listener that [`check_client`] has already accepted (an upload, checked
    /// before its file is read).
    Checked { id: String },
    /// The edge node on its link: the link's mutual TLS is its identity.
    Link,
}

impl Client {
    /// The edge node's client id on this node: `pass_edge_client`, else `edge`.
    pub fn edge_client_id() -> String {
        let id = moqui_lib::env::get_prop_or_env(EDGE_CLIENT_PROPERTY);
        match id.trim() {
            "" => DEFAULT_EDGE_CLIENT.to_string(),
            id => id.to_string(),
        }
    }
}

/// Why a call was not run: the client is not a registered one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused(pub String);

/// Check a direct client's id and secret on this thread (a request worker): its id, or [`Refused`].
///
/// # Errors
///
/// Returns [`Refused`] when they are not a registered client's.
pub fn check_client(id: &str, secret: &str) -> Result<String, Refused> {
    if registered(id, secret) {
        Ok(id.to_string())
    } else {
        Err(Refused(format!(
            "{id} is not a registered PASS client, or its secret is wrong"
        )))
    }
}

/// The session attributes that bind an authority session to the client and site it was made for: a handle is
/// good only for those.
const CLIENT_ATTRIBUTE: &str = "passClientId";
const SITE_ATTRIBUTE: &str = "passSiteId";
/// The label of the key a handle is signed with, derived from `session_cookie_key`, so a handle's MAC is never a
/// session cookie's, as the MCP requestState's is not.
const HANDLE_LABEL: &str = "moqui pass authority session handle v1";
/// The prefix of a handle, which makes it no `moqui.session` cookie value even in form.
const HANDLE_PREFIX: &str = "p1.";

fn handle_mac(id: &str) -> Result<hmac::Hmac<sha2::Sha256>, Error> {
    use hmac::{KeyInit as _, Mac as _};
    let key = crate::user::session::derived_cookie_key(HANDLE_LABEL).map_err(|err| Error::new(err.message()))?;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(&key)
        .map_err(|_| Error::new("session_cookie_key cannot be used as an HMAC key"))?;
    mac.update(id.as_bytes());
    Ok(mac)
}

/// The handle of the authority session `id`: `p1.{id}.{mac}` under the derived key.
pub fn sign_handle(id: &str) -> Result<String, Error> {
    use base64::Engine as _;
    use hmac::Mac as _;
    let tag = handle_mac(id)?.finalize().into_bytes();
    Ok(format!(
        "{HANDLE_PREFIX}{id}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(tag)
    ))
}

/// The session id a handle names, when its MAC is this server's; `None` for anything else, a session cookie too.
pub fn verify_handle(handle: &str) -> Option<String> {
    use base64::Engine as _;
    use hmac::Mac as _;
    let (id, tag) = handle.strip_prefix(HANDLE_PREFIX)?.split_once('.')?;
    let tag = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(tag).ok()?;
    handle_mac(id).ok()?.verify_slice(&tag).ok()?;
    Some(id.to_string())
}

/// Bind the authority session of an account action: the handle's, when it is this server's and was made for this
/// client and site, else a new one marked with them.
fn pass_session(call: &Call, client_id: &str) -> Result<crate::user::session::SessionGuard, Error> {
    use crate::user::session;
    let factory = ExecutionContext::with_current(|ctx| Arc::clone(ctx.factory()))
        .ok_or_else(|| Error::new("no execution context"))?;
    let owner = (Some(client_id.to_string()), Some(call.site_id.clone()));
    if let Some(id) = verify_handle(&call.session_handle) {
        let guard = session::bind_id(&factory, Some(&id), true).map_err(|err| Error::new(err.message()))?;
        if (
            session::session_attribute(CLIENT_ATTRIBUTE),
            session::session_attribute(SITE_ATTRIBUTE),
        ) == owner
        {
            return Ok(guard);
        }
        tracing::warn!(
            "PASS authority call {}: a session handle of another client or site, or of no PASS session; a new session \
             is used",
            call.operation_id
        );
        drop(guard);
    }
    let guard = session::bind_id(&factory, None, true).map_err(|err| Error::new(err.message()))?;
    session::set_session_attribute(CLIENT_ATTRIBUTE, Some(client_id));
    session::set_session_attribute(SITE_ATTRIBUTE, Some(&call.site_id));
    Ok(guard)
}

/// Run one call on this thread (a request worker with a fresh context, no user): the client's check, then the authority
/// service, with the authority session bound for an account action.
///
/// # Errors
///
/// Returns [`Refused`] when a direct client's id and secret are not a registered client's.
pub fn serve(call: &Call, client: &Client, file: Option<UploadFile>) -> Result<Reply, Refused> {
    let outcome = run(call, client, file)?;
    Ok(Reply {
        result: outcome.result,
        session_handle: outcome.session_handle,
    })
}

/// Run a download's call (`getAttachment`, `getExport`): the file's metadata and content, or the error envelope.
///
/// # Errors
///
/// Returns [`Refused`] as [`serve`] does.
pub fn serve_download(
    factory: &ExecutionContextFactory,
    call: &Call,
    client: &Client,
) -> Result<Result<Served, PassError>, Refused> {
    let outcome = run(call, client, None)?;
    if let Err(error) = outcome.result {
        return Ok(Err(error));
    }
    let Some(download) = outcome.download else {
        return Ok(Err(logged_unavailable(call, "the authority answered with no file")));
    };
    let location = download["location"].as_str().unwrap_or_default();
    let opened = match factory.resource().open(location) {
        Ok(Some(opened)) => opened,
        Ok(None) => return Ok(Err(logged_unavailable(call, "the file is not there"))),
        Err(err) => return Ok(Err(logged_unavailable(call, err.message()))),
    };
    let text = |name: &str| download[name].as_str().unwrap_or_default().to_string();
    // a ranged file (the authority sends its size) answers its range, checked again against the file as it is here
    let complete = opened.source.len();
    let ranged = download
        .get("size")
        .is_some_and(|size| !size.is_null())
        .then(|| Ranged::of(&download["range"], complete));
    let (start, length) = match ranged {
        Some(Ranged::Part { start, end, .. }) => (start, end - start + 1),
        Some(Ranged::Unsatisfiable { .. }) => (0, 0),
        Some(Ranged::Whole) | None => (0, complete),
    };
    Ok(Ok(Served {
        meta: DownloadMeta {
            content_type: match text("contentType") {
                kind if kind.is_empty() => "application/octet-stream".to_string(),
                kind => kind,
            },
            filename: text("filename"),
            length: Some(length),
            disposition: match text("disposition") {
                disposition if disposition == "inline" => disposition,
                _ => "attachment".to_string(),
            },
            ranged,
            cache_control: download_cache_control(&call.operation, &text("cacheControl")).map(str::to_string),
        },
        source: opened.source,
        start,
    }))
}

/// What the authority service answered.
struct Outcome {
    result: Result<serde_json::Value, PassError>,
    /// A file to send: `{location, contentType, filename, disposition}`, with `size` and `range` (`{start, end}`,
    /// `{unsatisfiable}` or null) for a file that takes byte ranges and `cacheControl` for a public image or theme asset.
    download: Option<serde_json::Value>,
    session_handle: String,
}

fn run(call: &Call, client: &Client, file: Option<UploadFile>) -> Result<Outcome, Refused> {
    let client_id = match client {
        Client::Secret { id, secret } => {
            if !registered(id, secret) {
                return Err(Refused(format!(
                    "{id} is not a registered PASS client, or its secret is wrong"
                )));
            }
            id.clone()
        }
        Client::Checked { id } => id.clone(),
        Client::Link => Client::edge_client_id(),
    };
    // An account flow's state (the second factor) is the authority session's, which the handle carries. The SSO back
    // channel keeps its state in its handoff rows, so it has none.
    let account = call.operation.starts_with(ACCOUNT_PREFIX) && !call.operation.starts_with("account.sso-");
    let session = if account {
        match pass_session(call, &client_id) {
            Ok(guard) => Some(guard),
            Err(err) => {
                return Ok(error_outcome(logged_unavailable(
                    call,
                    &format!("the authority session could not be bound: {}", err.message()),
                )));
            }
        }
    } else {
        None
    };
    let mut parameters = BTreeMap::new();
    let text = |value: &str| Value::string(value);
    parameters.insert("operation".to_string(), text(&call.operation));
    parameters.insert("siteId".to_string(), text(&call.site_id));
    parameters.insert(
        "input".to_string(),
        match from_json(&call.input) {
            map @ Value::Map(_) => map,
            _ => Value::Map(indexmap::IndexMap::new()),
        },
    );
    parameters.insert("delegation".to_string(), text(&call.delegation));
    parameters.insert("clientId".to_string(), text(&client_id));
    parameters.insert("clientVerified".to_string(), Value::Bool(true));
    parameters.insert("visitorAddress".to_string(), text(&call.visitor_address));
    parameters.insert("operationId".to_string(), text(&call.operation_id));
    if let Some(file) = file {
        let upload =
            crate::web::multipart::UploadedFile::in_memory("file", &file.filename, &file.content_type, file.bytes);
        parameters.insert("file".to_string(), upload.into_value());
    }
    let service = moqui_lib::env::get_prop_or_env(SERVICE_PROPERTY);
    let service = match service.trim() {
        "" => DEFAULT_SERVICE,
        name => name,
    };
    let answered = crate::service::sync(service, &parameters);
    let errors = ExecutionContext::with_current(|ctx| {
        let errors = ctx.message().errors().to_vec();
        ctx.message().clear_errors();
        errors
    })
    .unwrap_or_default();
    let mut outcome = match answered {
        Ok(_) if !errors.is_empty() => error_outcome(logged_unavailable(call, &errors.join("; "))),
        Ok(out) => {
            let json = |name: &str| out.get(name).map(to_json).filter(|json| !json.is_null());
            match (json("error"), json("result")) {
                (Some(error), _) => error_outcome(PassError::from_json(&error)),
                (None, result) => Outcome {
                    result: Ok(result.unwrap_or_else(|| serde_json::json!({}))),
                    download: json("download"),
                    session_handle: String::new(),
                },
            }
        }
        Err(err) => error_outcome(logged_unavailable(call, err.message())),
    };
    if let Some(guard) = session {
        // Once a delegation is issued the frontend signs in with it; the session keeps no user, so a handle carries
        // only a sign-in in progress (the second factor). Not logoutUser: that would end the user's other sessions.
        if outcome
            .result
            .as_ref()
            .is_ok_and(|result| result.get("credential").is_some())
        {
            crate::user::session::clear_login();
            crate::user::session::set_session_attribute("moquiLastAuth", None);
        }
        if let Err(err) = crate::user::session::end_request() {
            tracing::warn!(
                "PASS authority call {}: the session was not saved: {err}",
                call.operation_id
            );
        }
        match sign_handle(&guard.session_id()) {
            Ok(handle) => outcome.session_handle = handle,
            Err(err) => tracing::warn!("PASS authority call {}: no session handle: {err}", call.operation_id),
        }
    }
    Ok(outcome)
}

fn error_outcome(error: PassError) -> Outcome {
    Outcome {
        result: Err(error),
        download: None,
        session_handle: String::new(),
    }
}

/// The REST API's 503 for a failure the client must not see the detail of, logged under a correlation id.
fn logged_unavailable(call: &Call, detail: &str) -> PassError {
    let correlation_id = crate::user::random_string(24).unwrap_or_default();
    tracing::warn!(
        "PASS request {correlation_id} failed (Unavailable): {} (operation {} {})",
        detail.lines().next().unwrap_or_default(),
        call.operation,
        call.operation_id
    );
    PassError {
        correlation_id,
        ..PassError::unavailable("Authority temporarily unavailable")
    }
}

/// Whether `id` with `secret` is a registered PASS client, by the framework's check (authorization off for it).
fn registered(id: &str, secret: &str) -> bool {
    if id.is_empty() || secret.is_empty() {
        return false;
    }
    let parameters = BTreeMap::from([
        ("clientId".to_string(), Value::string(id)),
        ("clientSecret".to_string(), Value::string(secret)),
    ]);
    let answered = {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        crate::service::sync(CHECK_CLIENT, &parameters)
    };
    match answered {
        Ok(out) => out.get("registered") == Some(&Value::Bool(true)),
        Err(err) => {
            tracing::error!("PASS authority: the client check {CHECK_CLIENT} failed: {err}");
            false
        }
    }
}

/// A gRPC authority's reply for a script, the shape of the REST adapter's: `{result}` or `{error}`, with
/// `sessionHandle`.
pub fn reply_json(reply: &Reply) -> serde_json::Value {
    let mut object = serde_json::Map::new();
    match &reply.result {
        Ok(result) => {
            object.insert("result".to_string(), result.clone());
        }
        Err(error) => {
            object.insert("error".to_string(), error.to_json());
        }
    }
    object.insert(
        "sessionHandle".to_string(),
        serde_json::Value::String(reply.session_handle.clone()),
    );
    serde_json::Value::Object(object)
}

#[cfg(test)]
#[path = "pass_authority_tests.rs"]
mod tests;
