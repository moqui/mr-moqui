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

//! Ranking texts a caller holds against a query by meaning: the semantic find of a page
//! (`ec.search.rankTexts`, the webroot `find/rank` transition, `MoquiFind.js`). Nothing is stored; the vectors of
//! the texts are kept in memory ([`VectorCache`]) so a page is embedded once and each query embeds only itself.
//!
//! A text's score is its cosine to the query, with [`WORD_WEIGHT`] of it the share of the query's words the text
//! has, so a text that has every word of the query is ahead of one that only means the same.

use std::collections::HashMap;
use std::sync::Arc;

use moqui_lib::error::Error;

use super::semantic::Embedder;

/// Texts one call ranks.
pub const MAX_TEXTS: usize = 5000;

/// Characters of a text that are embedded.
pub const MAX_CHARS: usize = 2000;

/// Vectors the cache keeps before it starts again.
const CACHE_SIZE: usize = 20_000;

/// The share of the score that is the query's words in the text.
pub const WORD_WEIGHT: f32 = 0.2;

/// A text of the call, by its position in `texts`.
#[derive(Clone, Debug, PartialEq)]
pub struct RankedText {
    pub index: usize,
    pub score: f32,
    /// The cosine of the text and the query.
    pub meaning: f32,
    /// The share of the query's words the text has.
    pub words: f32,
}

/// The vectors of texts by model and text. Full, it is emptied: a page's texts are embedded again on their next call.
#[derive(Default)]
pub struct VectorCache {
    vectors: HashMap<(String, String), Arc<Vec<f32>>>,
}

impl VectorCache {
    /// The vectors of `texts` (each with `prefix` before it when embedded), embedding the ones not kept yet in one
    /// call of `embedder`.
    ///
    /// # Errors
    ///
    /// Returns the model's error, or an error when it gives another number of vectors than texts.
    pub fn vectors(&mut self, embedder: &dyn Embedder, prefix: &str, texts: &[String]) -> Result<Vec<Arc<Vec<f32>>>, Error> {
        let model = embedder.model().to_string();
        let mut missing: Vec<String> = Vec::new();
        for text in texts {
            if !self.vectors.contains_key(&(model.clone(), text.clone())) && !missing.contains(text) {
                missing.push(text.clone());
            }
        }
        if !missing.is_empty() {
            let input: Vec<String> = missing.iter().map(|text| format!("{prefix}{text}")).collect();
            let vectors = embedder.embed(&input)?;
            if vectors.len() != missing.len() {
                return Err(Error::new(format!(
                    "the model gave {} vectors for {} texts",
                    vectors.len(),
                    missing.len()
                )));
            }
            if self.vectors.len() + missing.len() > CACHE_SIZE {
                self.vectors.clear();
            }
            for (text, vector) in missing.into_iter().zip(vectors) {
                self.vectors.insert((model.clone(), text), Arc::new(vector));
            }
        }
        Ok(texts
            .iter()
            .map(|text| self.vectors.get(&(model.clone(), text.clone())).cloned().unwrap_or_default())
            .collect())
    }
}

/// `text` cut to [`MAX_CHARS`] characters.
pub fn clip(text: &str) -> String {
    text.chars().take(MAX_CHARS).collect()
}

/// The texts by score, best first, at most `limit`; ties keep their order in `texts`.
pub fn rank(query: &str, query_vector: &[f32], texts: &[String], vectors: &[Arc<Vec<f32>>], limit: usize) -> Vec<RankedText> {
    let words = query_words(query);
    let mut ranked: Vec<RankedText> = texts
        .iter()
        .zip(vectors)
        .enumerate()
        .map(|(index, (text, vector))| {
            let meaning = cosine(query_vector, vector);
            let words = word_share(&words, text);
            RankedText {
                index,
                score: (1.0 - WORD_WEIGHT) * meaning + WORD_WEIGHT * words,
                meaning,
                words,
            }
        })
        .collect();
    ranked.sort_by(|left, right| right.score.total_cmp(&left.score).then(left.index.cmp(&right.index)));
    ranked.truncate(limit);
    ranked
}

/// The cosine of two vectors; 0 when either is empty, zero, or of another length.
pub fn cosine(left: &[f32], right: &[f32]) -> f32 {
    if left.is_empty() || left.len() != right.len() {
        return 0.0;
    }
    let (mut dot, mut left_norm, mut right_norm) = (0.0f32, 0.0f32, 0.0f32);
    for (a, b) in left.iter().zip(right) {
        dot += a * b;
        left_norm += a * a;
        right_norm += b * b;
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        return 0.0;
    }
    dot / (left_norm.sqrt() * right_norm.sqrt())
}

/// The lower-case words of a query, of three letters or more.
fn query_words(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in words_of(query) {
        if word.chars().count() >= 3 && !out.contains(&word) {
            out.push(word);
        }
    }
    out
}

/// The share of `words` a word of `text` starts with ("pump" finds "pumps").
fn word_share(words: &[String], text: &str) -> f32 {
    if words.is_empty() {
        return 0.0;
    }
    let text_words = words_of(text);
    let found = words
        .iter()
        .filter(|word| text_words.iter().any(|text_word| text_word.starts_with(word.as_str())))
        .count();
    #[allow(clippy::cast_precision_loss)] // counts of words in one query
    let share = found as f32 / words.len() as f32;
    share
}

fn words_of(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::semantic::HashEmbedder;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A [`HashEmbedder`] that counts the texts it embeds.
    struct Counting {
        inner: HashEmbedder,
        texts: AtomicUsize,
    }

    impl Embedder for Counting {
        fn model(&self) -> &str {
            "counting"
        }
        fn dimensions(&self) -> usize {
            self.inner.dimensions()
        }
        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, Error> {
            self.texts.fetch_add(texts.len(), Ordering::SeqCst);
            self.inner.embed(texts)
        }
    }

    fn strings(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|text| (*text).to_string()).collect()
    }

    #[test]
    fn cache_embeds_each_text_once() {
        let embedder = Counting {
            inner: HashEmbedder::new(64),
            texts: AtomicUsize::new(0),
        };
        let mut cache = VectorCache::default();
        let texts = strings(&["the pump", "a valve", "the pump"]);
        let first = cache.vectors(&embedder, "", &texts).unwrap();
        assert_eq!(embedder.texts.load(Ordering::SeqCst), 2);
        assert_eq!(first[0], first[2]);
        cache.vectors(&embedder, "", &strings(&["a valve", "a hose"])).unwrap();
        assert_eq!(embedder.texts.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn rank_puts_the_text_with_the_query_words_first() {
        let embedder = HashEmbedder::new(256);
        let texts = strings(&[
            "Every order ships the same day.",
            "A part I fit in a pump stays as it is, and a part with a service record needs less of a warranty check.",
            "A pump ships with its manual.",
        ]);
        let mut cache = VectorCache::default();
        let vectors = cache.vectors(&embedder, "", &texts).unwrap();
        let query = embedder.embed(&strings(&["pump and warranty"])).unwrap().pop().unwrap();
        let ranked = rank("pump and warranty", &query, &texts, &vectors, 2);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].index, 1);
        assert!((ranked[0].words - 1.0).abs() < f32::EPSILON);
        assert_eq!(ranked[1].index, 2);
    }

    #[test]
    fn word_share_matches_word_starts() {
        let words = query_words("Pumps of a farm");
        assert_eq!(words, strings(&["pumps", "farm"]));
        assert!((word_share(&words, "the farm's pumpset and pumps") - 1.0).abs() < f32::EPSILON);
        assert!((word_share(&words, "farms") - 0.5).abs() < f32::EPSILON);
        assert!(word_share(&words, "nothing here").abs() < f32::EPSILON);
    }

    #[test]
    fn cosine_of_mismatched_vectors_is_zero() {
        assert!(cosine(&[1.0, 0.0], &[1.0]).abs() < f32::EPSILON);
        assert!(cosine(&[0.0, 0.0], &[1.0, 0.0]).abs() < f32::EPSILON);
        assert!((cosine(&[1.0, 0.0], &[2.0, 0.0]) - 1.0).abs() < 1e-6);
    }
}
