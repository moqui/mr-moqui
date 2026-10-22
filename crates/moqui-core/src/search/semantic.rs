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

//! Semantic retrieval: embeddings of document text, stored beside each document, and searched by
//! nearest neighbor.
//!
//! The model is behind [`Embedder`]. No model is built in; nothing calls a model server unless it is
//! configured: `embedder="llm"` (or `llm:<modelId>`) is the LLM embedder
//! (`crate::llm::embed::LlmEmbedder`, with the `llm` feature), which calls the `/v1/embeddings` of
//! the model the `llm-facade` binds to the `embed` feature (or names), a local embedding model in
//! LM Studio, Ollama or llama.cpp being the expected use. [`HashEmbedder`] (`embedder="hash"`) is a
//! deterministic bag-of-words hash for tests and development: it exercises storage, the worker,
//! and the search path, and it has no semantic quality. The
//! Postgres backend stores vectors with the `pgvector` extension, which embedded Postgres does not
//! bundle; without it, or without an embedder, the backend does not list
//! [`super::Capability::Semantic`].

use std::sync::Arc;

use moqui_lib::error::Error;
use moqui_lib::node::MNode;
use moqui_lib::object::{to_plain_string, Value};

use super::mapping::DocumentMapping;
use crate::entity::document::Document;

/// Text in, one vector per text out, all of [`Self::dimensions`].
pub trait Embedder: Send + Sync {
    /// The model's name, stored with each vector so vectors of another model are not compared.
    fn model(&self) -> &str;
    fn dimensions(&self) -> usize;
    /// # Errors
    ///
    /// Returns an error when the model cannot run.
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, Error>;

    /// [`Self::embed`] for texts a person waits on (a query): with a short limit where the model is remote.
    ///
    /// # Errors
    ///
    /// Returns an error when the model cannot run.
    fn embed_quick(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, Error> {
        self.embed(texts)
    }

    /// Whether a batch that failed with `error` is worth sending again a text at a time, to find the texts the model
    /// fails on: not when the model's server is not answering.
    fn retries_each(&self, _error: &Error) -> bool {
        true
    }
}

/// Feature hashing of lower-cased words into `dimensions` buckets, normalized. Documents that share
/// words are near each other. For tests and development only.
pub struct HashEmbedder {
    dimensions: usize,
}

impl HashEmbedder {
    pub fn new(dimensions: usize) -> Self {
        Self {
            dimensions: dimensions.max(1),
        }
    }
}

impl Embedder for HashEmbedder {
    fn model(&self) -> &str {
        "moqui-hash"
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, Error> {
        Ok(texts
            .iter()
            .map(|text| {
                let mut vector = vec![0.0_f32; self.dimensions];
                for word in text
                    .split(|ch: char| !ch.is_alphanumeric())
                    .filter(|word| !word.is_empty())
                {
                    let hash = moqui_lib::codec::fnv1a(word.to_lowercase().as_bytes());
                    let slot = usize::try_from(hash % self.dimensions as u64).unwrap_or(0);
                    vector[slot] += 1.0;
                }
                let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
                if norm > 0.0 {
                    for value in &mut vector {
                        *value /= norm;
                    }
                }
                vector
            })
            .collect())
    }
}

/// The `semantic` element of `search-facade`: off unless `enabled="true"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticConf {
    pub enabled: bool,
    pub dimensions: usize,
    /// `halfvec` (16-bit, half the storage) or `vector`.
    pub representation: String,
    /// Characters of document text an embedding is made from.
    pub max_chars: usize,
    /// The embedder to install: `llm` is the LLM embedder of the `embed` feature and
    /// `llm:<modelId>` of that model; `hash` is [`HashEmbedder`], for development only. An
    /// application may install another with `SearchFacade::set_embedder`.
    pub embedder: Option<String>,
}

impl Default for SemanticConf {
    fn default() -> Self {
        Self {
            enabled: false,
            dimensions: 1024,
            representation: "halfvec".to_string(),
            max_chars: 2000,
            embedder: None,
        }
    }
}

impl SemanticConf {
    /// Read `search-facade/semantic`. No element is disabled.
    ///
    /// # Errors
    ///
    /// Returns an error for a representation other than `halfvec` or `vector`, or a bad number.
    pub fn from_conf(search_facade: &MNode) -> Result<Self, Error> {
        let Some(node) = search_facade.first_named("semantic") else {
            return Ok(Self::default());
        };
        let text = |name: &str| {
            node.attribute(name)
                .map(|raw| moqui_lib::env::expand(&raw).unwrap_or(raw).trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let mut conf = Self {
            enabled: text("enabled").is_some_and(|value| value == "true"),
            ..Self::default()
        };
        if let Some(value) = text("dimensions") {
            conf.dimensions = value
                .parse()
                .map_err(|_| Error::new(format!("semantic dimensions {value} is not a whole number")))?;
        }
        if let Some(value) = text("representation") {
            if value != "halfvec" && value != "vector" {
                return Err(Error::new(format!(
                    "semantic representation {value} is not halfvec or vector"
                )));
            }
            conf.representation = value;
        }
        conf.embedder = text("embedder");
        if let Some(value) = text("max-chars") {
            conf.max_chars = value
                .parse()
                .map_err(|_| Error::new(format!("semantic max-chars {value} is not a whole number")))?;
        }
        Ok(conf)
    }
}

/// A document waiting for an embedding: its id, the stored `indexed_at` the body is from (the
/// stale guard), and the body.
#[derive(Clone, Debug)]
pub struct PendingEmbedding {
    pub id: String,
    pub seen: String,
    pub body: Document,
}

/// The text an embedding is made from: the title, subtitle, code, and category roles first, then
/// the rest of the document's strings, up to `max_chars`.
pub fn embedding_text(mapping: &DocumentMapping, body: &Document, max_chars: usize) -> String {
    let mut out = String::new();
    for role in ["title", "subtitle", "code", "category"] {
        if let Some(value) = mapping.display.role(role).and_then(|name| body.get(name)) {
            if !value.is_null() {
                out.push_str(&to_plain_string(value));
                out.push_str(". ");
            }
        }
    }
    let mut rest = Vec::new();
    strings(&Value::Map(body.clone()), &mut rest);
    for text in rest {
        if out.len() >= max_chars {
            break;
        }
        if !out.contains(&text) {
            out.push_str(&text);
            out.push(' ');
        }
    }
    out.chars().take(max_chars).collect::<String>().trim().to_string()
}

fn strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Map(map) => {
            for item in map.values() {
                strings(item, out);
            }
        }
        Value::List(items) => {
            for item in items {
                strings(item, out);
            }
        }
        Value::String(text) if !text.trim().is_empty() => out.push(text.clone()),
        _ => {}
    }
}

/// An installed embedder, shared.
pub type SharedEmbedder = Arc<dyn Embedder>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_embedder_is_deterministic_and_normalized() {
        let embedder = HashEmbedder::new(64);
        let out = embedder
            .embed(&["Water Pump".to_string(), "water pump".to_string(), String::new()])
            .unwrap();
        assert_eq!(out[0], out[1]);
        let norm: f32 = out[0].iter().map(|value| value * value).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
        assert!(out[2].iter().all(|value| *value == 0.0));
    }

    #[test]
    fn conf_is_off_without_the_element() {
        let root = MNode::parse_text("search-facade", "<search-facade/>").unwrap();
        assert!(!SemanticConf::from_conf(&root).unwrap().enabled);
        let root = MNode::parse_text(
            "search-facade",
            r#"<search-facade><semantic enabled="true" dimensions="384" representation="vector"/></search-facade>"#,
        )
        .unwrap();
        let conf = SemanticConf::from_conf(&root).unwrap();
        assert!(conf.enabled);
        assert_eq!(conf.dimensions, 384);
        assert_eq!(conf.representation, "vector");
    }
}
