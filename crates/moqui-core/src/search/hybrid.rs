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

//! Hybrid and semantic retrieval, run by the facade over any backend.
//!
//! Hybrid search runs bounded branches, each with the request's trusted filter, and fuses them by
//! reciprocal rank: a hit's score is the sum over branches of `weight / (60 + rank)`, keyed by
//! document type and id together. The branches are:
//!
//! - lexical: the request's query, or the text as words when the query is `match_all`;
//! - prefix: `suggest` on the title, when the backend has autocomplete (the exact-match branch,
//!   weighted highest by default);
//! - fuzzy: `fuzzy` on the title, only when the lexical branch matched fewer than `fuzzy_below`;
//! - semantic: nearest neighbors of the text's embedding, when an embedder is installed and the
//!   backend has semantic search. No embedding, no branch.
//!
//! The total is exact over the documents the non-semantic branches match, so a hit found only by
//! meaning can be on the page and not in the total. Facets count that same population. A
//! semantic-only search reports its candidate count instead (`TotalRelation::Candidates`).

use std::collections::HashMap;
use std::sync::Arc;

use moqui_lib::error::Error;
use moqui_lib::object::to_plain_string;

use super::highlight::highlights;
use super::mapping::DocumentMapping;
use super::query::{Query, TextFields, TextMatch};
use super::semantic::Embedder;
use super::{Capability, SearchBackend, SearchHit, SearchPage, SearchRequest, TotalRelation, MAX_WINDOW};

/// The reciprocal rank fusion constant.
pub const RRF_K: f64 = 60.0;

/// Branch weights. Defaults favor an exact title prefix and keep typo recovery modest; they are a
/// starting point to measure, not constants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BranchWeights {
    pub lexical: f64,
    pub prefix: f64,
    pub fuzzy: f64,
    pub semantic: f64,
}

impl Default for BranchWeights {
    fn default() -> Self {
        Self {
            lexical: 1.0,
            prefix: 2.0,
            fuzzy: 0.5,
            semantic: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HybridOptions {
    /// What the person typed. The prefix, fuzzy, and semantic branches use it.
    pub text: String,
    /// Candidates each branch contributes.
    pub k: usize,
    pub weights: BranchWeights,
    /// Run the fuzzy branch only when the lexical branch matched fewer documents than this.
    pub fuzzy_below: u64,
    /// Added to a fused score per unit of the `quality` role's value, read as 0 to 1. 0 is off.
    pub quality_boost: f64,
}

impl Default for HybridOptions {
    fn default() -> Self {
        Self {
            text: String::new(),
            k: 50,
            weights: BranchWeights::default(),
            fuzzy_below: 5,
            quality_boost: 0.0,
        }
    }
}

/// Candidates each branch reads: `k`, at least the requested page, at most [`MAX_WINDOW`].
fn branch_window(request: &SearchRequest, options: &HybridOptions) -> usize {
    options
        .k
        .max(request.offset.saturating_add(request.limit))
        .clamp(1, MAX_WINDOW)
}

fn branch_request(request: &SearchRequest, query: Query, limit: usize) -> SearchRequest {
    let mut branch = SearchRequest::new(query);
    branch.limit = limit;
    branch.filter = request.filter.clone();
    branch.fresh_by = request.fresh_by;
    branch.rank = request.rank;
    branch
}

/// The lexical branch's query: the request's, or the text as words.
pub fn lexical_query(request: &SearchRequest, options: &HybridOptions) -> Query {
    if request.query == Query::MatchAll && !options.text.trim().is_empty() {
        Query::Text {
            terms: options.text.trim().to_string(),
            fields: TextFields::All,
            kind: TextMatch::Term,
        }
    } else {
        request.query.clone()
    }
}

/// A hybrid search.
///
/// # Errors
///
/// Returns an error when a branch fails or the embedder cannot run.
pub fn search(
    backend: &dyn SearchBackend,
    mappings: &[Arc<DocumentMapping>],
    request: &SearchRequest,
    options: &HybridOptions,
    embedder: Option<&dyn Embedder>,
) -> Result<SearchPage, Error> {
    let have = backend.capabilities();
    let text = options.text.trim();
    let k = branch_window(request, options);
    let lexical = lexical_query(request, options);
    let lexical_page = backend.search(mappings, &branch_request(request, lexical.clone(), k))?;
    let mut counted = vec![lexical.clone()];
    let mut branches = vec![(options.weights.lexical, lexical_page.hits)];
    if !text.is_empty() && have.contains(&Capability::Autocomplete) {
        let prefix = Query::Suggest { text: text.to_string() };
        let page = backend.search(mappings, &branch_request(request, prefix.clone(), k))?;
        branches.push((options.weights.prefix, page.hits));
        counted.push(prefix);
    }
    if !text.is_empty() && have.contains(&Capability::Fuzzy) && lexical_page.total < options.fuzzy_below {
        let fuzzy = Query::Fuzzy {
            terms: text.to_string(),
        };
        let page = backend.search(mappings, &branch_request(request, fuzzy.clone(), k))?;
        branches.push((options.weights.fuzzy, page.hits));
        counted.push(fuzzy);
    }
    if let Some(embedder) = embedder.filter(|_| !text.is_empty() && have.contains(&Capability::Semantic)) {
        let vector = embedder.embed(&[text.to_string()])?.pop().unwrap_or_default();
        let hits = backend.semantic_search(mappings, &vector, embedder.model(), request.filter.as_ref(), k)?;
        branches.push((options.weights.semantic, hits));
    }

    let fused = fuse(branches, mappings, options.quality_boost);
    let count = SearchRequest {
        limit: 0,
        ..branch_request(request, Query::any(counted), 0)
    };
    let total = backend.search(mappings, &count)?.total;
    Ok(page_of(fused, request, &lexical, total, TotalRelation::Eq))
}

/// A semantic-only search: the nearest neighbors of the text's embedding.
///
/// # Errors
///
/// Returns an error when the embedder cannot run or the backend has no semantic search.
pub fn semantic_search(
    backend: &dyn SearchBackend,
    mappings: &[Arc<DocumentMapping>],
    request: &SearchRequest,
    options: &HybridOptions,
    embedder: &dyn Embedder,
) -> Result<SearchPage, Error> {
    let k = branch_window(request, options);
    let vector = embedder
        .embed(&[options.text.trim().to_string()])?
        .pop()
        .unwrap_or_default();
    let hits = backend.semantic_search(mappings, &vector, embedder.model(), request.filter.as_ref(), k)?;
    let total = u64::try_from(hits.len()).unwrap_or(u64::MAX);
    Ok(page_of(
        hits,
        request,
        &Query::MatchAll,
        total,
        TotalRelation::Candidates,
    ))
}

/// Reciprocal rank fusion, then the quality boost, best first; ties by document type, then id.
pub fn fuse(
    branches: Vec<(f64, Vec<SearchHit>)>,
    mappings: &[Arc<DocumentMapping>],
    quality_boost: f64,
) -> Vec<SearchHit> {
    let mut scores: HashMap<(String, String), (f64, SearchHit)> = HashMap::new();
    for (weight, hits) in branches {
        for (index, hit) in hits.into_iter().enumerate() {
            let add = weight / (RRF_K + (index + 1) as f64);
            scores
                .entry((hit.document_id.clone(), hit.id.clone()))
                .and_modify(|entry| entry.0 += add)
                .or_insert((add, hit));
        }
    }
    let mut out: Vec<SearchHit> = scores
        .into_values()
        .map(|(mut score, mut hit)| {
            if quality_boost > 0.0 {
                score += quality_boost * quality(mappings, &hit);
            }
            hit.score = score;
            hit
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.document_id.cmp(&b.document_id))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// The `quality` role's value of a hit, as 0 to 1. Missing or unreadable is 0.
fn quality(mappings: &[Arc<DocumentMapping>], hit: &SearchHit) -> f64 {
    mappings
        .iter()
        .find(|mapping| mapping.document_id == hit.document_id)
        .and_then(|mapping| mapping.display.role("quality"))
        .and_then(|name| hit.body.get(name))
        .and_then(|value| to_plain_string(value).parse::<f64>().ok())
        .map_or(0.0, |value| value.clamp(0.0, 1.0))
}

fn page_of(
    hits: Vec<SearchHit>,
    request: &SearchRequest,
    highlight_query: &Query,
    total: u64,
    total_relation: TotalRelation,
) -> SearchPage {
    let hits = hits
        .into_iter()
        .skip(request.offset)
        .take(request.limit)
        .map(|mut hit| {
            hit.highlights = if request.highlight.is_empty() {
                Default::default()
            } else {
                highlights(highlight_query, &request.highlight, &hit.body)
            };
            hit
        })
        .collect();
    SearchPage {
        hits,
        total,
        total_relation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::document::Document;

    fn hit(document_id: &str, id: &str) -> SearchHit {
        SearchHit {
            document_id: document_id.into(),
            id: id.into(),
            index: String::new(),
            score: 0.0,
            body: Document::default(),
            highlights: Default::default(),
        }
    }

    #[test]
    fn fusion_sums_reciprocal_ranks_by_type_and_id() {
        let fused = fuse(
            vec![
                (1.0, vec![hit("A", "1"), hit("A", "2"), hit("B", "1")]),
                (2.0, vec![hit("A", "2")]),
                (1.0, vec![hit("B", "1"), hit("A", "1")]),
            ],
            &[],
            0.0,
        );
        let order: Vec<(String, String)> = fused
            .iter()
            .map(|hit| (hit.document_id.clone(), hit.id.clone()))
            .collect();
        // A/2: 1/62 + 2/61; A/1: 1/61 + 1/62; B/1 (a different document from A/1): 1/63 + 1/61.
        assert_eq!(
            order,
            vec![
                ("A".to_string(), "2".to_string()),
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "1".to_string())
            ]
        );
        assert!((fused[0].score - (1.0 / 62.0 + 2.0 / 61.0)).abs() < 1e-12);
    }

    #[test]
    fn branch_window_covers_the_page_within_the_search_window() {
        let mut request = SearchRequest::new(Query::MatchAll);
        request.offset = 40;
        request.limit = 20;
        let options = |k| HybridOptions {
            k,
            ..HybridOptions::default()
        };
        assert_eq!(branch_window(&request, &options(50)), 60);
        assert_eq!(branch_window(&request, &options(100)), 100);
        assert_eq!(branch_window(&request, &options(usize::MAX)), MAX_WINDOW);
        request.offset = usize::MAX;
        assert_eq!(branch_window(&request, &options(0)), MAX_WINDOW);
    }
}
