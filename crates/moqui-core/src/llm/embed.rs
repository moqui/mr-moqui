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

//! Embeddings through a configured model (plans/LLM-A2A-Assist.md 4.2): [`LlmFacade::embed`] posts texts to the
//! OpenAI-style `/v1/embeddings` of the model's endpoint (a Chat Completions or Responses endpoint: LM Studio, Ollama,
//! llama.cpp, vLLM, OpenAI), at most [`BATCH`] texts a request, each request logged as an `LlmCall` of source `embed`.
//! [`LlmEmbedder`] is the [`Embedder`] of the search facade (`search-facade semantic embedder="llm"`, or
//! `embedder="llm:<modelId>"`) and of the skill retrieval's semantic branch when the `embed` feature is bound and the
//! search facade has no embedder of its own ([`bound_embedder`]). Nothing is sent unless one of these is configured;
//! local embedding models are the expected use.
//!
//! The request body is `{"model", "input"}` with the model's `api-config` merged onto it (`{"dimensions": 1024}` for a
//! model that shortens its vectors); the vectors come back in the order of `index`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_lib::error::Error;
use serde_json::{json, Map, Value as Json};

use super::conf::{EndpointConf, ProtocolId};
use super::provider::{self, pump, CancelToken, ErrorClass, ProviderError};
use super::registry::{Target, Want};
use super::usage::{CallRecord, CallSource, Usage};
use super::LlmFacade;
use crate::context::ExecutionContextFactory;
use crate::entity::EntityFacade;
use crate::search::semantic::Embedder;

/// Texts in one embeddings request.
pub const BATCH: usize = 64;

/// The limit of one embeddings request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// The URL of an endpoint's embeddings API: its path with a last `chat/completions` or `responses` replaced by
/// `embeddings`, else `/v1/embeddings` on its base URL.
pub fn embeddings_url(endpoint: &EndpointConf) -> String {
    let base = endpoint.url.trim_end_matches('/');
    let path = endpoint.path.as_deref().map(|path| path.trim_end_matches('/')).and_then(|path| {
        path.strip_suffix("chat/completions")
            .or_else(|| path.strip_suffix("responses"))
            .map(|prefix| format!("{prefix}embeddings"))
    });
    match path {
        Some(path) => format!("{base}/{}", path.trim_start_matches('/')),
        None => format!("{base}/v1/embeddings"),
    }
}

/// Whether the endpoint has the OpenAI embeddings API.
pub fn has_embeddings(endpoint: &EndpointConf) -> bool {
    matches!(endpoint.protocol, ProtocolId::ChatCompletions | ProtocolId::Responses)
}

/// What one embeddings request answered.
#[derive(Clone, Debug)]
pub(crate) struct Posted {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub first_byte_ms: Option<i64>,
}

/// POST `body` to `url` on `target` and read the whole response (at most `max` bytes), with `limits`. `None` when
/// `cancel` was set first.
///
/// # Errors
///
/// Returns the error of a request that could not be sent or read.
pub(crate) fn post(
    target: &Target,
    url: &str,
    body: &Json,
    limits: &pump::Limits,
    cancel: &CancelToken,
    max: usize,
) -> Result<Option<Posted>, ProviderError> {
    let started = Instant::now();
    let Some((status, headers, stream)) = provider::open_json(target, url, body, "application/json", limits, cancel)?
    else {
        return Ok(None);
    };
    let first_byte_ms = Some(i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX));
    let (text, end) = provider::read_capped(stream, limits, cancel, max);
    match end {
        pump::End::Done | pump::End::Stopped => Ok(Some(Posted {
            status,
            headers,
            body: text,
            first_byte_ms,
        })),
        pump::End::Canceled => Ok(None),
        pump::End::Failed(message) => Err(ProviderError::new(ErrorClass::Network, message)),
        end => Err(ProviderError::new(
            ErrorClass::Timeout,
            format!("the response of the model server did not end in time ({end:?})"),
        )),
    }
}

/// The usage of an embeddings response: its `prompt_tokens` (or `total_tokens`) as input.
pub fn usage_of(response: &Json) -> Usage {
    let usage = response.get("usage").cloned().unwrap_or(Json::Null);
    let mut out = provider::chat_completions::usage_of(&usage, None);
    if out.input == 0 {
        out.input = usage.get("total_tokens").and_then(Json::as_u64).unwrap_or(0);
    }
    out
}

/// The vectors of an embeddings response, in the order of their `index`.
///
/// # Errors
///
/// Returns why the response is not a list of `count` float vectors.
pub fn vectors_of(response: &Json, count: usize) -> Result<Vec<Vec<f32>>, String> {
    let data = response
        .get("data")
        .and_then(Json::as_array)
        .ok_or("the embeddings response has no data list")?;
    let mut out: Vec<Option<Vec<f32>>> = vec![None; count];
    for (at, item) in data.iter().enumerate() {
        let index = item
            .get("index")
            .and_then(Json::as_u64)
            .and_then(|index| usize::try_from(index).ok())
            .unwrap_or(at);
        let vector = item
            .get("embedding")
            .and_then(Json::as_array)
            .ok_or_else(|| format!("embedding {index} is not a list of numbers"))?
            .iter()
            .map(|value| value.as_f64().map(|value| value as f32))
            .collect::<Option<Vec<f32>>>()
            .ok_or_else(|| format!("embedding {index} has a value that is not a number"))?;
        let slot = out
            .get_mut(index)
            .ok_or_else(|| format!("embedding {index} is past the {count} texts sent"))?;
        *slot = Some(vector);
    }
    out.into_iter()
        .enumerate()
        .map(|(index, vector)| vector.ok_or_else(|| format!("no embedding came back for text {index}")))
        .collect()
}

/// The body of an embeddings request for `target`: the model's `api-config` with the model and the input over it.
pub fn request_body(target: &Target, input: Json) -> Json {
    let mut body = Map::new();
    if let Some(Json::Object(config)) = &target.model.api_config {
        provider::deep_merge(&mut body, config);
    }
    body.insert("model".to_string(), Json::String(target.model.name.clone()));
    body.insert("input".to_string(), input);
    Json::Object(body)
}

/// What [`LlmFacade::embed_batches`] made: the vectors of the batches that succeeded, in order from the first text,
/// and the error of the batch that did not (the later texts were not sent).
#[derive(Debug, Default)]
pub struct Batches {
    pub vectors: Vec<Vec<f32>>,
    pub error: Option<ProviderError>,
}

impl LlmFacade {
    /// Embed `texts` with the model of `target`, [`BATCH`] at a time, each request logged as an `LlmCall` of source
    /// `embed` for the current user. Out: one vector per text, in order.
    ///
    /// # Errors
    ///
    /// Returns an error when the facade is off, the endpoint has no embeddings API, or a request fails or answers
    /// something that is not one vector per text.
    pub fn embed(&self, facade: &EntityFacade, target: &Target, texts: &[String]) -> Result<Vec<Vec<f32>>, Error> {
        let batches = self.embed_batches(facade, target, texts, REQUEST_TIMEOUT)?;
        match batches.error {
            None => Ok(batches.vectors),
            Some(error) => Err(embed_error(target, &error)),
        }
    }

    /// [`Self::embed`] that keeps what the batches before a failed one made, each request with `timeout`.
    ///
    /// # Errors
    ///
    /// Returns an error when the facade is off or the model cannot embed at all (no embeddings API, no name).
    pub fn embed_batches(
        &self,
        facade: &EntityFacade,
        target: &Target,
        texts: &[String],
        timeout: Duration,
    ) -> Result<Batches, Error> {
        if !self.enabled() {
            return Err(Error::new("the llm-facade is disabled"));
        }
        if !has_embeddings(&target.endpoint) {
            return Err(Error::new(format!(
                "endpoint {} ({}) has no embeddings API; bind the embed feature to a model on a chat-completions or responses endpoint",
                target.endpoint.id,
                target.endpoint.protocol.as_str()
            )));
        }
        if target.model.name.is_empty() {
            return Err(Error::new(format!("model {} has no provider model name", target.model.id)));
        }
        let user_id = crate::user::current_user_id().ok().flatten().filter(|id| !id.is_empty());
        let url = embeddings_url(&target.endpoint);
        let mut out = Batches::default();
        for batch in texts.chunks(BATCH) {
            let body = request_body(target, json!(batch));
            let limits = pump::Limits::new(timeout, Some(Instant::now() + timeout));
            let mut record = CallRecord::new(CallSource::Embed, target);
            record.user_id.clone_from(&user_id);
            if self.conf().content_log.enabled {
                record.request_json = Some(body.to_string());
            }
            let started = Instant::now();
            let answer = post(target, &url, &body, &limits, &CancelToken::new(), provider::MAX_JSON_BODY);
            record.duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
            let outcome: Result<Vec<Vec<f32>>, ProviderError> = match answer {
                Ok(None) => Err(ProviderError::new(ErrorClass::Timeout, "the embeddings request was canceled")),
                Err(error) => Err(error),
                Ok(Some(posted)) => {
                    record.http_status = Some(posted.status);
                    record.first_token_ms = posted.first_byte_ms;
                    if self.conf().content_log.enabled {
                        record.response_json = Some(posted.body.clone());
                    }
                    if (200..300).contains(&posted.status) {
                        serde_json::from_str::<Json>(&posted.body)
                            .map_err(|error| ProviderError::new(ErrorClass::Server, format!("the response is not JSON: {error}")))
                            .and_then(|response| {
                                record.usage = usage_of(&response);
                                vectors_of(&response, batch.len())
                                    .map_err(|problem| ProviderError::new(ErrorClass::Server, problem))
                            })
                    } else {
                        let mut error = provider::classify::classify_http(posted.status, &posted.headers, &posted.body);
                        error.status = Some(posted.status);
                        Err(error)
                    }
                }
            };
            match &outcome {
                Ok(_) => record.finish = Some("stop".to_string()),
                Err(error) => {
                    record.finish = Some("error".to_string());
                    record.error_class = Some(error.class.as_str().to_string());
                    record.error_text = Some(error.to_string());
                }
            }
            if let Err(error) = self.record_call(facade, target, &record) {
                tracing::error!("the embeddings call to model {} was not logged: {error}", target.model.id);
            }
            match outcome {
                Ok(vectors) => out.vectors.extend(vectors),
                Err(error) => {
                    out.error = Some(error);
                    break;
                }
            }
        }
        Ok(out)
    }
}

fn embed_error(target: &Target, error: &ProviderError) -> Error {
    Error::new(format!(
        "the embeddings call to model {} failed: {}",
        target.model.id,
        super::usage::redact(&error.to_string(), &[])
    ))
}

/// Whether an error says the server is not answering (as opposed to refusing one input): the breaker opens on it.
fn is_outage(error: &ProviderError) -> bool {
    !matches!(
        error.class,
        ErrorClass::BadRequest | ErrorClass::ContextOverflow | ErrorClass::ContentFilter
    )
}

/// How long the semantic branch is skipped after the embeddings server did not answer.
pub const BREAKER: Duration = Duration::from_secs(60);

/// The limit of an embeddings request a person waits on (a query).
pub const QUICK_TIMEOUT: Duration = Duration::from_secs(8);

/// The embedder names whose server did not answer, and until when they are not called.
static OPEN_BREAKERS: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

fn breaker_open(name: &str) -> Option<Instant> {
    let mut open = OPEN_BREAKERS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let open = open.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    open.retain(|_, until| *until > now);
    open.get(name).copied()
}

fn open_breaker(name: &str) {
    let mut open = OPEN_BREAKERS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    open.get_or_insert_with(HashMap::new).insert(name.to_string(), Instant::now() + BREAKER);
}

/// Vectors an embedder keeps, by text; it is cleared when it holds this many.
const KEPT: usize = 4096;

/// Which model an [`LlmEmbedder`] calls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EmbedWith {
    /// The model bound to a feature (`embed`), with its fallback.
    Feature(String),
    Model(String),
}

impl EmbedWith {
    /// `llm` is the `embed` feature, `llm:<modelId>` that model; anything else is `None`.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "llm" => Some(EmbedWith::Feature("embed".to_string())),
            other => other
                .strip_prefix("llm:")
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(|model| EmbedWith::Model(model.to_string())),
        }
    }
}

/// The name of the vectors of `target`'s model.
fn name_of(target: &Target) -> String {
    format!("llm:{}/{}", target.model.id, target.model.name)
}

/// The [`Embedder`] over a configured model. Its [`Embedder::model`] is `llm:<modelId>/<provider model>`, taken when
/// it is made, so vectors of another model are never compared with its own: the binding is resolved again on each
/// call, and a call whose binding now goes to another model (a changed binding, its fallback) is refused rather than
/// stored under this name. The vectors of successful batches are kept by text, so a batch that failed is sent again
/// without the texts that were embedded. When the server does not answer, the name's breaker opens for [`BREAKER`]:
/// calls are refused at once meanwhile, and a failed batch is not tried again a text at a time
/// ([`Embedder::retries_each`]). [`Embedder::embed_quick`] has the short [`QUICK_TIMEOUT`].
pub struct LlmEmbedder {
    llm: Arc<LlmFacade>,
    facade: EntityFacade,
    with: EmbedWith,
    name: String,
    /// The configured dimensions, else those of the last vectors.
    dimensions: AtomicUsize,
    timeout: Duration,
    kept: Mutex<HashMap<String, Vec<f32>>>,
}

impl LlmEmbedder {
    /// An embedder calling the model `with` names. `dimensions` is what the caller stores (the search facade's
    /// `dimensions`); `None` is what the model answers.
    pub fn new(llm: Arc<LlmFacade>, facade: EntityFacade, with: EmbedWith, dimensions: Option<usize>) -> Self {
        let name = match Self::target_of(&llm, &facade, &with) {
            Ok(target) => name_of(&target),
            Err(error) => {
                tracing::error!("the LLM embedder ({with:?}) has no model yet: {error}");
                match &with {
                    EmbedWith::Feature(feature) => format!("llm:{feature}"),
                    EmbedWith::Model(model) => format!("llm:{model}"),
                }
            }
        };
        Self {
            llm,
            facade,
            with,
            name,
            dimensions: AtomicUsize::new(dimensions.unwrap_or(0)),
            timeout: REQUEST_TIMEOUT,
            kept: Mutex::new(HashMap::new()),
        }
    }

    /// The embedder of the factory's `llm-facade` for `with`, when the facade is on and the entity facade installed.
    pub fn for_factory(factory: &ExecutionContextFactory, with: EmbedWith, dimensions: Option<usize>) -> Option<Self> {
        let llm = super::facade(factory);
        let facade = factory.entity()?;
        llm.enabled().then(|| Self::new(llm, facade, with, dimensions))
    }

    fn target_of(llm: &LlmFacade, facade: &EntityFacade, with: &EmbedWith) -> Result<Target, Error> {
        let want = match with {
            EmbedWith::Feature(feature) => Want {
                feature: Some(feature),
                ..Want::default()
            },
            EmbedWith::Model(model) => Want {
                model: Some(model),
                ..Want::default()
            },
        };
        llm.resolve(facade, &want)
    }

    fn embed_within(&self, texts: &[String], timeout: Duration) -> Result<Vec<Vec<f32>>, Error> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        if let Some(until) = breaker_open(&self.name) {
            return Err(Error::new(format!(
                "the embeddings server of {} did not answer a moment ago; it is not called for {} s more",
                self.name,
                until.saturating_duration_since(Instant::now()).as_secs()
            )));
        }
        let target = Self::target_of(&self.llm, &self.facade, &self.with)?;
        let resolved = name_of(&target);
        if resolved != self.name {
            return Err(Error::new(format!(
                "the embedder's model is {} but its binding now goes to {resolved}, whose vectors are not comparable; make \
                 the embedder again (a restart) and embed the documents again",
                self.name
            )));
        }
        let missing: Vec<String> = {
            let kept = self.kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut missing: Vec<String> = texts.iter().filter(|text| !kept.contains_key(*text)).cloned().collect();
            missing.dedup();
            missing
        };
        let batches = self.llm.embed_batches(&self.facade, &target, &missing, timeout)?;
        {
            let mut kept = self.kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if kept.len() + batches.vectors.len() > KEPT {
                kept.clear();
            }
            for (text, vector) in missing.iter().zip(&batches.vectors) {
                kept.insert(text.clone(), vector.clone());
            }
        }
        if let Some(error) = &batches.error {
            if is_outage(error) {
                open_breaker(&self.name);
            }
            return Err(embed_error(&target, error));
        }
        let kept = self.kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let vectors: Vec<Vec<f32>> = texts
            .iter()
            .map(|text| kept.get(text).cloned())
            .collect::<Option<_>>()
            .ok_or_else(|| Error::new("an embedding that came back was not kept"))?;
        if let Some(first) = vectors.first() {
            let _ = self
                .dimensions
                .compare_exchange(0, first.len(), Ordering::Relaxed, Ordering::Relaxed);
        }
        Ok(vectors)
    }
}

impl Embedder for LlmEmbedder {
    fn model(&self) -> &str {
        &self.name
    }

    fn dimensions(&self) -> usize {
        self.dimensions.load(Ordering::Relaxed)
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, Error> {
        self.embed_within(texts, self.timeout)
    }

    fn embed_quick(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, Error> {
        self.embed_within(texts, QUICK_TIMEOUT.min(self.timeout))
    }

    fn retries_each(&self, _error: &Error) -> bool {
        breaker_open(&self.name).is_none()
    }
}

/// The LLM embedder of the `embed` feature, when the `llm-facade` is on and binds `embed` (in the conf or a
/// `LlmFeatureBinding` row): the semantic branch of the skill retrieval when the search facade has no embedder. One
/// embedder is kept per factory and binding, so what it embedded is kept between queries.
pub fn bound_embedder(factory: &ExecutionContextFactory) -> Option<Arc<LlmEmbedder>> {
    let llm = super::facade(factory);
    if !llm.enabled() {
        return None;
    }
    let facade = factory.entity()?;
    let target = llm.registry(&facade).resolve_feature("embed").ok()?;
    let slot = factory.extension(|_| BoundEmbedder::default());
    let mut held = slot.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let name = name_of(&target);
    if held.as_ref().is_none_or(|embedder| embedder.name != name) {
        *held = Some(Arc::new(LlmEmbedder::new(llm, facade, EmbedWith::Feature("embed".to_string()), None)));
    }
    held.clone()
}

/// The factory's embedder of the `embed` feature ([`bound_embedder`]).
#[derive(Default)]
struct BoundEmbedder(Mutex<Option<Arc<LlmEmbedder>>>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embeddings_url_follows_the_endpoint_path() {
        let mut endpoint = EndpointConf::new("e", ProtocolId::ChatCompletions, "http://127.0.0.1:1234/");
        assert_eq!(embeddings_url(&endpoint), "http://127.0.0.1:1234/v1/embeddings");
        endpoint.path = Some("/v1beta/openai/chat/completions".to_string());
        assert_eq!(embeddings_url(&endpoint), "http://127.0.0.1:1234/v1beta/openai/embeddings");
        endpoint.path = Some("/api/responses".to_string());
        assert_eq!(embeddings_url(&endpoint), "http://127.0.0.1:1234/api/embeddings");
        endpoint.path = Some("/custom".to_string());
        assert_eq!(embeddings_url(&endpoint), "http://127.0.0.1:1234/v1/embeddings");
    }

    #[test]
    fn vectors_come_back_in_index_order_and_a_missing_one_is_an_error() {
        let response = json!({"data": [
            {"index": 1, "embedding": [0.5, 0.25]},
            {"index": 0, "embedding": [1, 0]}
        ], "usage": {"prompt_tokens": 7, "total_tokens": 7}});
        assert_eq!(vectors_of(&response, 2).unwrap(), vec![vec![1.0, 0.0], vec![0.5, 0.25]]);
        assert_eq!(usage_of(&response).input, 7);
        assert!(vectors_of(&response, 3).unwrap_err().contains("text 2"));
        assert!(vectors_of(&json!({"data": [{"index": 5, "embedding": [1]}]}), 1).is_err());
        assert!(vectors_of(&json!({"object": "list"}), 1).is_err());
    }

    #[test]
    fn the_embedder_names_parse() {
        assert_eq!(EmbedWith::parse("llm"), Some(EmbedWith::Feature("embed".to_string())));
        assert_eq!(EmbedWith::parse("llm:nomic"), Some(EmbedWith::Model("nomic".to_string())));
        assert_eq!(EmbedWith::parse("llm:"), None);
        assert_eq!(EmbedWith::parse("hash"), None);
    }
}
