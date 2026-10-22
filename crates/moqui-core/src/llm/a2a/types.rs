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

//! The A2A 1.0 wire types the server reads and writes (the `a2a.proto` messages as their proto JSON, camelCase, enums
//! by name): the errors of both bindings, the version check, the checks of a message and its parts, the request
//! parameters (history length, page size, the list cursor), and the JSON of a task, a status, a message and an
//! artifact. No database here; [`super::ops`] reads the rows.
//!
//! The SDKs parse a result strictly (an unknown field is an error), so every object written here has only the fields
//! of its message; what is Moqui's own goes in `metadata` or in a `data` part.

use base64::Engine;
use serde_json::{json, Map, Value as Json};

/// The protocol version served, and the one the `A2A-Version` header must name.
pub const VERSION: &str = "1.0";
/// The header of the version (A2A 1.0, 3.6.1); a request without it is 0.3, which is not served.
pub const VERSION_HEADER: &str = "A2A-Version";
/// The artifact a run's answer is streamed into and kept as.
pub const RESPONSE_ARTIFACT: &str = "response";
/// The output modes a task can answer in.
pub const OUTPUT_MODES: &[&str] = &["text/plain", "application/json"];
/// ListTasks page size without one, and the most it takes.
pub const DEFAULT_PAGE_SIZE: usize = 50;
pub const MAX_PAGE_SIZE: usize = 100;
/// History messages of a task returned without a `historyLength`.
pub const MAX_HISTORY: usize = 200;

const ERROR_INFO: &str = "type.googleapis.com/google.rpc.ErrorInfo";
const DOMAIN: &str = "a2a-protocol.org";

/// An error of either binding: its JSON-RPC code, the message the caller reads, and the `google.rpc.ErrorInfo` reason
/// of an A2A error. An internal failure's own text is logged, never sent ([`A2aError::internal`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct A2aError {
    pub code: i32,
    pub message: String,
}

impl A2aError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn parse(message: impl Into<String>) -> Self {
        Self::new(-32700, message)
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(-32600, message)
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(-32601, format!("Method {method} not found"))
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(-32602, message)
    }

    /// The caller reads only "Internal error"; the cause is the log's.
    pub fn internal() -> Self {
        Self::new(-32603, "Internal error")
    }

    pub fn task_not_found(task_id: &str) -> Self {
        Self::new(-32001, format!("Task {task_id} was not found"))
    }

    pub fn not_cancelable(state: &str) -> Self {
        Self::new(-32002, format!("Task cannot be canceled: it is {state}"))
    }

    pub fn push_not_supported(message: impl Into<String>) -> Self {
        Self::new(-32003, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(-32004, message)
    }

    pub fn content_type(message: impl Into<String>) -> Self {
        Self::new(-32005, message)
    }

    pub fn extended_card_not_configured() -> Self {
        Self::new(-32007, "the extended agent card is not configured")
    }

    pub fn version(message: impl Into<String>) -> Self {
        Self::new(-32009, message)
    }

    /// The `ErrorInfo` reason of an A2A error, as the SDKs map it back.
    pub fn reason(&self) -> &'static str {
        match self.code {
            -32700 => "JSON_PARSE",
            -32600 => "INVALID_REQUEST",
            -32601 => "METHOD_NOT_FOUND",
            -32602 => "INVALID_PARAMS",
            -32001 => "TASK_NOT_FOUND",
            -32002 => "TASK_NOT_CANCELABLE",
            -32003 => "PUSH_NOTIFICATION_NOT_SUPPORTED",
            -32004 => "UNSUPPORTED_OPERATION",
            -32005 => "CONTENT_TYPE_NOT_SUPPORTED",
            -32006 => "INVALID_AGENT_RESPONSE",
            -32007 => "EXTENDED_AGENT_CARD_NOT_CONFIGURED",
            -32008 => "EXTENSION_SUPPORT_REQUIRED",
            -32009 => "VERSION_NOT_SUPPORTED",
            _ => "INTERNAL_ERROR",
        }
    }

    /// The HTTP status of the error in the HTTP+JSON binding, and its `google.rpc.Code` name.
    pub fn http_status(&self) -> (u16, &'static str) {
        match self.code {
            -32001 => (404, "NOT_FOUND"),
            -32601 => (404, "NOT_FOUND"),
            -32002 | -32003 | -32004 | -32007 | -32008 | -32009 => (400, "FAILED_PRECONDITION"),
            -32700 | -32600 | -32602 | -32005 => (400, "INVALID_ARGUMENT"),
            _ => (500, "INTERNAL"),
        }
    }

    fn details(&self) -> Json {
        json!([{"@type": ERROR_INFO, "reason": self.reason(), "domain": DOMAIN, "metadata": {}}])
    }

    /// The JSON-RPC `error` member.
    pub fn to_rpc(&self) -> Json {
        json!({"code": self.code, "message": self.message, "data": self.details()})
    }

    /// The HTTP+JSON error body, a `google.rpc.Status`.
    pub fn to_rest(&self) -> Json {
        let (status, name) = self.http_status();
        json!({"error": {"code": status, "status": name, "message": self.message, "details": self.details()}})
    }
}

impl std::fmt::Display for A2aError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

/// The `A2A-Version` a request names: 1.0 (or a 1.0 patch). None or empty is 0.3, which is not served.
///
/// # Errors
///
/// Returns VersionNotSupported (`-32009`) for any other.
pub fn check_version(header: Option<&str>) -> Result<(), A2aError> {
    let asked = header.map(str::trim).unwrap_or_default();
    if asked.is_empty() {
        return Err(A2aError::version(
            "the A2A-Version header is required; without it a request is 0.3, which is not served (served: 1.0)",
        ));
    }
    let patch = asked
        .strip_prefix("1.0.")
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()));
    if asked == VERSION || patch {
        Ok(())
    } else {
        Err(A2aError::version(format!(
            "A2A-Version {asked} is not served (served: 1.0)"
        )))
    }
}

/// The caps on a message's parts (`a2a` `max-parts`, `max-part-bytes`).
#[derive(Clone, Copy, Debug)]
pub struct PartLimits {
    pub max_parts: usize,
    pub max_part_bytes: usize,
}

/// A message a client sent, checked: its ids, its parts (each exactly one of `text`, `raw`, `url` and `data`), the
/// text the model reads of it, its data (a lone data part's value, for an answer to a form) and the `dbresource://`
/// locations its `url` parts name (checked as existing by the caller, never fetched).
#[derive(Clone, Debug, PartialEq)]
pub struct InMessage {
    pub message_id: String,
    pub context_id: Option<String>,
    pub task_id: Option<String>,
    pub reference_task_ids: Vec<String>,
    pub parts: Vec<Json>,
    pub metadata: Option<Json>,
    pub text: String,
    pub data: Option<Json>,
    pub locations: Vec<String>,
}

fn opt_id(message: &Map<String, Json>, field: &str) -> Result<Option<String>, A2aError> {
    match message.get(field) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(text)) if !text.trim().is_empty() => Ok(Some(text.trim().to_string())),
        Some(_) => Err(A2aError::invalid_params(format!(
            "message.{field} must be a non-blank string"
        ))),
    }
}

fn string_list(value: Option<&Json>, path: &str) -> Result<Vec<String>, A2aError> {
    match value {
        None | Some(Json::Null) => Ok(Vec::new()),
        Some(Json::Array(items)) => items
            .iter()
            .map(|item| match item.as_str().map(str::trim) {
                Some(text) if !text.is_empty() => Ok(text.to_string()),
                _ => Err(A2aError::invalid_params(format!(
                    "{path} must be an array of non-blank strings"
                ))),
            })
            .collect(),
        Some(_) => Err(A2aError::invalid_params(format!(
            "{path} must be an array of non-blank strings"
        ))),
    }
}

fn object_or_null(value: Option<&Json>, path: &str) -> Result<Option<Json>, A2aError> {
    match value {
        None | Some(Json::Null) => Ok(None),
        Some(object @ Json::Object(_)) => Ok(Some(object.clone())),
        Some(_) => Err(A2aError::invalid_params(format!("{path} must be an object"))),
    }
}

/// Whether a part's `url` is a location of this server's own content, which is the only kind taken.
pub fn content_location(location: &str) -> bool {
    location.starts_with("dbresource://")
}

/// The decoded length of a `raw` part's base64 (standard, else URL-safe).
fn raw_length(text: &str) -> Option<usize> {
    let engines = [
        base64::engine::general_purpose::STANDARD,
        base64::engine::general_purpose::URL_SAFE,
    ];
    engines
        .iter()
        .find_map(|engine| engine.decode(text.trim()).ok())
        .or_else(|| {
            base64::engine::general_purpose::STANDARD_NO_PAD
                .decode(text.trim())
                .ok()
        })
        .or_else(|| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(text.trim())
                .ok()
        })
        .map(|bytes| bytes.len())
}

/// A message of a `SendMessage`, checked. Only a user's message is taken.
///
/// # Errors
///
/// Returns InvalidParams naming what does not fit.
pub fn parse_message(value: Option<&Json>, limits: PartLimits) -> Result<InMessage, A2aError> {
    let Some(Json::Object(message)) = value else {
        return Err(A2aError::invalid_params("message is required and must be an object"));
    };
    let message_id =
        opt_id(message, "messageId")?.ok_or_else(|| A2aError::invalid_params("message.messageId is required"))?;
    if message_id.chars().count() > 255 {
        return Err(A2aError::invalid_params(
            "message.messageId is longer than 255 characters",
        ));
    }
    match message.get("role").and_then(Json::as_str) {
        Some("ROLE_USER") => {}
        Some("ROLE_AGENT") => {
            return Err(A2aError::invalid_params(
                "SendMessage takes a message with role ROLE_USER",
            ))
        }
        _ => return Err(A2aError::invalid_params("message.role must be ROLE_USER")),
    }
    let parts = match message.get("parts") {
        Some(Json::Array(parts)) if !parts.is_empty() => parts,
        _ => return Err(A2aError::invalid_params("message.parts must have at least one Part")),
    };
    if parts.len() > limits.max_parts {
        return Err(A2aError::invalid_params(format!(
            "message.parts has more than {} parts",
            limits.max_parts
        )));
    }
    let mut chunks = Vec::new();
    let mut data = None;
    let mut locations = Vec::new();
    let mut kept = Vec::new();
    for part in parts {
        let Json::Object(fields) = part else {
            return Err(A2aError::invalid_params("each Part must be an object"));
        };
        let variants: Vec<&str> = ["text", "raw", "url", "data"]
            .into_iter()
            .filter(|key| fields.contains_key(*key))
            .collect();
        if variants.len() != 1 {
            return Err(A2aError::invalid_params(
                "each Part must have exactly one of text, raw, url and data",
            ));
        }
        for (field, kind) in [("filename", "a string"), ("mediaType", "a string")] {
            if fields
                .get(field)
                .is_some_and(|value| !value.is_string() && !value.is_null())
            {
                return Err(A2aError::invalid_params(format!("Part.{field} must be {kind}")));
            }
        }
        object_or_null(fields.get("metadata"), "Part.metadata")?;
        let filename = fields.get("filename").and_then(Json::as_str).unwrap_or("unnamed");
        let media = fields
            .get("mediaType")
            .and_then(Json::as_str)
            .map(|media| format!(" ({media})"))
            .unwrap_or_default();
        match variants[0] {
            "text" => {
                let Some(text) = fields.get("text").and_then(Json::as_str) else {
                    return Err(A2aError::invalid_params("Part.text must be a string"));
                };
                if text.len() > limits.max_part_bytes {
                    return Err(A2aError::invalid_params(format!(
                        "Part.text is over {} bytes",
                        limits.max_part_bytes
                    )));
                }
                chunks.push(text.to_string());
                kept.push(part.clone());
            }
            "raw" => {
                let length = fields
                    .get("raw")
                    .and_then(Json::as_str)
                    .filter(|text| !text.is_empty())
                    .and_then(raw_length)
                    .ok_or_else(|| A2aError::invalid_params("Part.raw must be base64 encoded bytes"))?;
                if length > limits.max_part_bytes {
                    return Err(A2aError::invalid_params(format!(
                        "Part.raw is over {} bytes",
                        limits.max_part_bytes
                    )));
                }
                // the bytes are not kept: the model and the history read what was attached
                let note = format!("[Attached file {filename}{media}, {length} bytes, not kept]");
                chunks.push(note.clone());
                kept.push(json!({"text": note}));
            }
            "url" => {
                let location = fields
                    .get("url")
                    .and_then(Json::as_str)
                    .map(str::trim)
                    .unwrap_or_default();
                if location.is_empty() {
                    return Err(A2aError::invalid_params("Part.url must be a non-blank string"));
                }
                if !content_location(location) {
                    return Err(A2aError::invalid_params(
                        "Part.url must be a dbresource:// location of this server; other URLs are not fetched",
                    ));
                }
                chunks.push(format!("[File {filename}{media} at {location}]"));
                locations.push(location.to_string());
                kept.push(part.clone());
            }
            _ => {
                let value = fields.get("data").cloned().unwrap_or(Json::Null);
                let text = value.to_string();
                if text.len() > limits.max_part_bytes {
                    return Err(A2aError::invalid_params(format!(
                        "Part.data is over {} bytes",
                        limits.max_part_bytes
                    )));
                }
                chunks.push(text);
                if data.is_none() {
                    data = Some(value);
                }
                kept.push(part.clone());
            }
        }
    }
    string_list(message.get("extensions"), "message.extensions")?;
    Ok(InMessage {
        message_id,
        context_id: opt_id(message, "contextId")?,
        task_id: opt_id(message, "taskId")?,
        reference_task_ids: string_list(message.get("referenceTaskIds"), "message.referenceTaskIds")?,
        parts: kept,
        metadata: object_or_null(message.get("metadata"), "message.metadata")?,
        text: chunks.join("\n"),
        data,
        locations,
    })
}

/// A push notification config as a client gives it: where to post, the token it echoes, and the authentication to
/// send (`{scheme, credentials}`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PushIn {
    pub id: Option<String>,
    pub task_id: Option<String>,
    pub url: String,
    pub token: Option<String>,
    pub authentication: Option<(String, Option<String>)>,
}

/// A `TaskPushNotificationConfig` (its fields at the top, or a 1.0 draft's nested `pushNotificationConfig`).
///
/// # Errors
///
/// Returns InvalidParams when it has no URL or a field of the wrong kind.
pub fn parse_push(value: &Json) -> Result<PushIn, A2aError> {
    let Json::Object(outer) = value else {
        return Err(A2aError::invalid_params(
            "the push notification config must be an object",
        ));
    };
    let mut fields = outer.clone();
    if let Some(Json::Object(inner)) = outer.get("pushNotificationConfig") {
        for (key, value) in inner {
            fields.insert(key.clone(), value.clone());
        }
    }
    let text = |field: &str| -> Result<Option<String>, A2aError> {
        match fields.get(field) {
            None | Some(Json::Null) => Ok(None),
            Some(Json::String(text)) => Ok(Some(text.trim().to_string()).filter(|text| !text.is_empty())),
            Some(_) => Err(A2aError::invalid_params(format!("{field} must be a string"))),
        }
    };
    let url = text("url")?.ok_or_else(|| A2aError::invalid_params("url is required"))?;
    let authentication = match fields.get("authentication") {
        None | Some(Json::Null) => None,
        Some(Json::Object(auth)) => {
            let scheme = auth
                .get("scheme")
                .and_then(Json::as_str)
                .map(str::trim)
                .unwrap_or_default();
            // a 1.0 draft listed schemes
            let scheme = if scheme.is_empty() {
                auth.get("schemes")
                    .and_then(Json::as_array)
                    .and_then(|schemes| schemes.first())
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .trim()
            } else {
                scheme
            };
            if scheme.is_empty() || scheme.contains(char::is_whitespace) {
                return Err(A2aError::invalid_params(
                    "authentication.scheme must be one word, such as Bearer",
                ));
            }
            let credentials = auth
                .get("credentials")
                .and_then(Json::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string);
            Some((scheme.to_string(), credentials))
        }
        Some(_) => return Err(A2aError::invalid_params("authentication must be an object")),
    };
    let token = text("token")?;
    for (name, value) in [
        ("token", token.as_deref()),
        (
            "credentials",
            authentication.as_ref().and_then(|auth| auth.1.as_deref()),
        ),
    ] {
        if value.is_some_and(|value| value.len() > 4096 || value.chars().any(char::is_control)) {
            return Err(A2aError::invalid_params(format!(
                "{name} must be at most 4096 characters, with no control characters"
            )));
        }
    }
    Ok(PushIn {
        id: text("id")?,
        task_id: text("taskId")?,
        url,
        token,
        authentication,
    })
}

/// The `SendMessageConfiguration` of a request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SendConfig {
    pub history_length: Option<usize>,
    pub return_immediately: bool,
    pub push: Option<PushIn>,
}

/// The configuration of a send, checked: some accepted output mode must be one a task answers in.
///
/// # Errors
///
/// Returns ContentTypeNotSupported or InvalidParams.
pub fn parse_config(value: Option<&Json>) -> Result<SendConfig, A2aError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(SendConfig::default());
    };
    let Json::Object(config) = value else {
        return Err(A2aError::invalid_params("configuration must be an object"));
    };
    let modes = string_list(config.get("acceptedOutputModes"), "configuration.acceptedOutputModes")?;
    if !modes.is_empty()
        && !modes
            .iter()
            .any(|mode| OUTPUT_MODES.contains(&mode.as_str()) || mode == "*/*" || mode == "text/*")
    {
        return Err(A2aError::content_type(format!(
            "none of the accepted output modes is served: {} (served: {})",
            modes.join(", "),
            OUTPUT_MODES.join(", ")
        )));
    }
    let return_immediately = match config.get("returnImmediately") {
        None | Some(Json::Null) => false,
        Some(Json::Bool(flag)) => *flag,
        Some(_) => {
            return Err(A2aError::invalid_params(
                "configuration.returnImmediately must be a boolean",
            ))
        }
    };
    let push = match config
        .get("taskPushNotificationConfig")
        .or_else(|| config.get("pushNotificationConfig"))
    {
        None | Some(Json::Null) => None,
        Some(push) => Some(parse_push(push)?),
    };
    Ok(SendConfig {
        history_length: history_length(config.get("historyLength"))?,
        return_immediately,
        push,
    })
}

fn non_negative(value: Option<&Json>, name: &str) -> Result<Option<i64>, A2aError> {
    let number = match value {
        None | Some(Json::Null) => return Ok(None),
        Some(Json::Number(number)) => number.as_i64(),
        // a query parameter of the HTTP+JSON binding
        Some(Json::String(text)) if !text.trim().is_empty() => text.trim().parse::<i64>().ok(),
        Some(Json::String(_)) => return Ok(None),
        Some(_) => None,
    };
    match number {
        Some(number) if number >= 0 => Ok(Some(number)),
        _ => Err(A2aError::invalid_params(format!("{name} must be zero or more"))),
    }
}

/// `historyLength`: none is the whole history (up to [`MAX_HISTORY`]), 0 none, n the newest n.
///
/// # Errors
///
/// Returns InvalidParams for a negative or non-numeric value.
pub fn history_length(value: Option<&Json>) -> Result<Option<usize>, A2aError> {
    Ok(non_negative(value, "historyLength")?.map(|n| usize::try_from(n).unwrap_or(MAX_HISTORY).min(MAX_HISTORY)))
}

/// `pageSize`: 1 to 100, 50 without one.
///
/// # Errors
///
/// Returns InvalidParams out of range.
pub fn page_size(value: Option<&Json>) -> Result<usize, A2aError> {
    match non_negative(value, "pageSize").map_err(|_| A2aError::invalid_params("pageSize must be 1 to 100"))? {
        None => Ok(DEFAULT_PAGE_SIZE),
        Some(size) if (1..=100).contains(&size) => Ok(usize::try_from(size).unwrap_or(DEFAULT_PAGE_SIZE)),
        Some(_) => Err(A2aError::invalid_params("pageSize must be 1 to 100")),
    }
}

/// A flag of a request (a query parameter is the text `true` or `false`).
pub fn flag(value: Option<&Json>) -> bool {
    match value {
        Some(Json::Bool(flag)) => *flag,
        Some(Json::String(text)) => text.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// An RFC 3339 time as epoch milliseconds.
///
/// # Errors
///
/// Returns InvalidParams when it is not one.
pub fn timestamp_millis(value: Option<&Json>, name: &str) -> Result<Option<i64>, A2aError> {
    match value {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(text)) if text.trim().is_empty() => Ok(None),
        Some(Json::String(text)) => text
            .trim()
            .parse::<jiff::Timestamp>()
            .map(|at| Some(at.as_millisecond()))
            .map_err(|_| A2aError::invalid_params(format!("{name} must be an RFC 3339 time"))),
        Some(_) => Err(A2aError::invalid_params(format!("{name} must be an RFC 3339 time"))),
    }
}

/// Epoch milliseconds as the proto JSON of a `Timestamp`.
pub fn rfc3339(millis: i64) -> String {
    jiff::Timestamp::from_millisecond(millis).map_or_else(|_| "1970-01-01T00:00:00Z".to_string(), |at| at.to_string())
}

/// The keyset cursor of ListTasks: the submitted time and run id of a page's last task, newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub submitted: i64,
    pub run_id: String,
}

impl Cursor {
    pub fn encode(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(json!({"v": 1, "s": self.submitted, "r": self.run_id}).to_string())
    }

    /// The cursor of a `pageToken`; empty is the first page.
    ///
    /// # Errors
    ///
    /// Returns InvalidParams for a token this server did not make.
    pub fn decode(token: Option<&str>) -> Result<Option<Self>, A2aError> {
        let Some(token) = token.map(str::trim).filter(|token| !token.is_empty()) else {
            return Ok(None);
        };
        let invalid = || A2aError::invalid_params("pageToken is not valid");
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(token)
            .map_err(|_| invalid())?;
        let value: Json = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        match (value["v"].as_i64(), value["s"].as_i64(), value["r"].as_str()) {
            (Some(1), Some(submitted), Some(run_id)) if !run_id.is_empty() => Ok(Some(Self {
                submitted,
                run_id: run_id.to_string(),
            })),
            _ => Err(invalid()),
        }
    }
}

/// A `TaskState` filter of ListTasks.
///
/// # Errors
///
/// Returns InvalidParams for a name that is not a task state.
pub fn state_filter(value: Option<&Json>) -> Result<Option<crate::llm::run::RunState>, A2aError> {
    match value.and_then(Json::as_str).map(str::trim) {
        None | Some("" | "TASK_STATE_UNSPECIFIED") => Ok(None),
        Some(name) => crate::llm::run::RunState::from_wire(name)
            .map(Some)
            .ok_or_else(|| A2aError::invalid_params(format!("{name} is not a task state"))),
    }
}

/// A text part.
pub fn text_part(text: &str) -> Json {
    json!({"text": text})
}

/// A data part of a JSON value.
pub fn data_part(data: Json) -> Json {
    json!({"data": data, "mediaType": "application/json"})
}

/// A `Message` of a task.
pub fn message(message_id: &str, context_id: &str, task_id: &str, agent: bool, parts: Vec<Json>) -> Json {
    json!({"messageId": message_id, "contextId": context_id, "taskId": task_id,
        "role": if agent { "ROLE_AGENT" } else { "ROLE_USER" }, "parts": parts})
}

/// The `response` artifact with its text.
pub fn response_artifact(text: &str) -> Json {
    json!({"artifactId": RESPONSE_ARTIFACT, "name": RESPONSE_ARTIFACT, "parts": [text_part(text)]})
}

/// A `TaskStatus`.
pub fn status(state: &str, message: Option<Json>, millis: Option<i64>) -> Json {
    let mut status = json!({"state": state});
    if let Some(message) = message {
        status["message"] = message;
    }
    if let Some(millis) = millis {
        status["timestamp"] = json!(rfc3339(millis));
    }
    status
}

/// A `StreamResponse` of a status change.
pub fn status_update(task_id: &str, context_id: &str, status: Json) -> Json {
    json!({"statusUpdate": {"taskId": task_id, "contextId": context_id, "status": status}})
}

/// A `StreamResponse` of a chunk of an artifact: `append` adds the parts to what the client has, else they replace it.
pub fn artifact_update(task_id: &str, context_id: &str, artifact: Json, append: bool, last_chunk: bool) -> Json {
    json!({"artifactUpdate": {"taskId": task_id, "contextId": context_id, "artifact": artifact, "append": append,
        "lastChunk": last_chunk}})
}

/// An id a client chose (a push config's): letters, digits, `-`, `_` and `.`, at most 40.
pub fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}
