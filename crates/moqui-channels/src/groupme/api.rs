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

//! GroupMe's REST API v3, its image service and its push server, as the adapter calls them: the token in the
//! `X-Access-Token` header (never in a URL), GroupMe's `{response, meta}` envelope, and its answers by class
//! (dev.groupme.com, "Responses": 304 nothing new, 400 bad data, 401 the token, 403 refused or update limits, 404, 420
//! rate limited, 5xx).

use std::io::Read as _;
use std::time::Duration;

use moqui_core::service::chat_bridge::DeliveryOutcome;
use moqui_lib::http::{Method, OpenRequest, RestClient};
use serde_json::Value as Json;

use super::Config;

/// Why a call to GroupMe did not answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    /// GroupMe answered this status, with its errors.
    Status { status: u16, errors: String },
    /// The request never reached GroupMe (a refused connection).
    Unreached(String),
    /// The request may have reached GroupMe, and its answer was lost.
    Lost(String),
}

impl ApiError {
    /// Whether GroupMe refused the token: the bridge is not connected until it has a new one.
    pub fn unauthorized(&self) -> bool {
        matches!(self, ApiError::Status { status: 401, .. })
    }

    /// Whether GroupMe asked for fewer requests (420 Enhance Your Calm, 429).
    pub fn rate_limited(&self) -> bool {
        matches!(self, ApiError::Status { status: 420 | 429, .. })
    }

    pub fn text(&self) -> String {
        match self {
            ApiError::Status { status, errors } if errors.is_empty() => format!("GroupMe answered {status}"),
            ApiError::Status { status, errors } => format!("GroupMe answered {status}: {errors}"),
            ApiError::Unreached(error) => format!("GroupMe could not be reached: {error}"),
            ApiError::Lost(error) => format!("GroupMe's answer was lost: {error}"),
        }
    }

    /// A send's outcome for this failure, `what` naming what did not go.
    pub fn outcome(&self, what: &str) -> DeliveryOutcome {
        let error = format!("{what}: {}", self.text());
        let failed = |reason: &str| DeliveryOutcome::Failed {
            reason_code: reason.to_string(),
            error: error.clone(),
        };
        match self {
            ApiError::Status { status: 401, .. } => DeliveryOutcome::Retry {
                error: format!("GroupMe refused the access token: get a new one (docs/interfaces/GroupMe.md). {error}"),
                not_connected: true,
            },
            ApiError::Status {
                status: 420 | 429 | 502 | 503,
                ..
            }
            | ApiError::Unreached(_) => DeliveryOutcome::Retry {
                error,
                not_connected: false,
            },
            ApiError::Status { status: 400, .. } => failed("badRequest"),
            ApiError::Status { status: 404, .. } => failed("notFound"),
            ApiError::Status { status, .. } if (400..500).contains(status) => failed("refused"),
            ApiError::Status { .. } | ApiError::Lost(_) => DeliveryOutcome::Uncertain { error },
        }
    }
}

/// An id field of a GroupMe object, a string or a number.
pub fn id_of(value: &Json, key: &str) -> Option<String> {
    match value.get(key)? {
        Json::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
        Json::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// GroupMe's errors of an answer (`meta.errors`), joined.
fn errors_of(body: &str) -> String {
    serde_json::from_str::<Json>(body)
        .ok()
        .and_then(|answer| {
            answer
                .get("meta")
                .and_then(|meta| meta.get("errors"))
                .and_then(Json::as_array)
                .map(|errors| errors.iter().filter_map(Json::as_str).collect::<Vec<_>>().join("; "))
        })
        .unwrap_or_default()
        .chars()
        .take(300)
        .collect()
}

/// Calls to GroupMe for one bridge.
pub struct Api<'a> {
    client: &'a RestClient,
    config: &'a Config,
}

impl<'a> Api<'a> {
    pub fn new(client: &'a RestClient, config: &'a Config) -> Self {
        Self { client, config }
    }

    fn token(&self) -> &str {
        self.config.token.as_deref().unwrap_or_default()
    }

    fn failure(&self, error: &moqui_lib::error::Error) -> ApiError {
        let text = self.config.scrub(error.message());
        if crate::sms::never_sent(error) {
            ApiError::Unreached(text)
        } else {
            ApiError::Lost(text)
        }
    }

    /// The `response` of an answer: a 2xx's (or 304's, as null); else the status and GroupMe's errors.
    fn read(&self, status: u16, body: &str) -> Result<Json, ApiError> {
        if status == 304 {
            return Ok(Json::Null);
        }
        if !(200..300).contains(&status) {
            return Err(ApiError::Status {
                status,
                errors: self.config.scrub(&errors_of(body)),
            });
        }
        if body.trim().is_empty() {
            return Ok(Json::Null);
        }
        let answer: Json = serde_json::from_str(body).map_err(|_| ApiError::Status {
            status: 502,
            errors: "GroupMe's answer is not JSON".to_string(),
        })?;
        Ok(answer.get("response").cloned().unwrap_or(answer))
    }

    /// `GET` of `path` (with its query) under the API's base.
    ///
    /// # Errors
    ///
    /// As [`ApiError`].
    pub fn get(&self, path: &str) -> Result<Json, ApiError> {
        let response = self
            .client
            .get(format!("{}{path}", self.config.api_base))
            .header("X-Access-Token", self.token())
            .header("Accept", "application/json")
            .send(false)
            .map_err(|error| self.failure(&error))?;
        self.read(response.status(), response.body())
    }

    /// `POST` of JSON `body` to `path` under the API's base.
    ///
    /// # Errors
    ///
    /// As [`ApiError`].
    pub fn post(&self, path: &str, body: &Json) -> Result<Json, ApiError> {
        let response = self
            .client
            .post(format!("{}{path}", self.config.api_base))
            .header("X-Access-Token", self.token())
            .json(body)
            .send(false)
            .map_err(|error| self.failure(&error))?;
        self.read(response.status(), response.body())
    }

    /// A picture to GroupMe's image service (`POST /pictures`, its bytes as the body): the `i.groupme.com` URL a
    /// message's `image` attachment names.
    ///
    /// # Errors
    ///
    /// As [`ApiError`]; an answer with no URL is a 502.
    pub fn upload_image(&self, bytes: Vec<u8>, content_type: &str) -> Result<String, ApiError> {
        let request = OpenRequest {
            method: Method::Post,
            url: format!("{}/pictures", self.config.image_base),
            headers: vec![
                ("X-Access-Token".to_string(), self.token().to_string()),
                ("Content-Type".to_string(), content_type.to_string()),
            ],
            body: Some(bytes.into()),
            timeout: Some(Duration::from_secs(60)),
        };
        let mut response = self.client.open(request).map_err(|error| self.failure(&error))?;
        let mut body = Vec::new();
        (&mut response.reader)
            .take(64 * 1024)
            .read_to_end(&mut body)
            .map_err(|error| ApiError::Lost(error.to_string()))?;
        let body = String::from_utf8_lossy(&body);
        if !(200..300).contains(&response.status) {
            return Err(ApiError::Status {
                status: response.status,
                errors: self.config.scrub(&errors_of(&body)),
            });
        }
        let answer: Json = serde_json::from_str(&body).unwrap_or(Json::Null);
        let payload = answer.get("payload").unwrap_or(&answer);
        payload
            .get("url")
            .or_else(|| payload.get("picture_url"))
            .and_then(Json::as_str)
            .filter(|url| url.starts_with("https://") || self.config.allow_internal)
            .map(str::to_string)
            .ok_or_else(|| ApiError::Status {
                status: 502,
                errors: "GroupMe's image service named no picture".to_string(),
            })
    }

    /// One exchange with the push server (Bayeux over HTTP): the messages it answers, waiting at most `wait`.
    ///
    /// # Errors
    ///
    /// As [`ApiError`]; an answer that is not a JSON list is a 502.
    pub fn faye(&self, messages: &Json, wait: Duration) -> Result<Vec<Json>, ApiError> {
        let response = self
            .client
            .post(self.config.push_url.clone())
            .json(messages)
            .timeout(wait)
            .send(false)
            .map_err(|error| self.failure(&error))?;
        if !(200..300).contains(&response.status()) {
            return Err(ApiError::Status {
                status: response.status(),
                errors: String::new(),
            });
        }
        match serde_json::from_str::<Json>(response.body()) {
            Ok(Json::Array(list)) => Ok(list),
            _ => Err(ApiError::Status {
                status: 502,
                errors: "the push server's answer is not a list".to_string(),
            }),
        }
    }
}
