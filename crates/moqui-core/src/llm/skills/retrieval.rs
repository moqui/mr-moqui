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

//! Skill retrieval (plan 7.5): Java's weights over the index, deterministic and with no model.
//!
//! The query is normalized (lower case, words), its intent and filler words are dropped as loose tokens (they still
//! count in a whole phrase), and each word is stemmed on the query side only (Carbon's catalog search: `accounts`
//! finds `account`, the skill's text is not stemmed) and widened by the component aliases (`skill/aliases.json`) at
//! half weight. A skill scores: the whole query as its name or title 100; the query inside its name or title 40; two
//! or more words as a phrase in its name, title or description 40; each word in its name or title, or one of its
//! front matter aliases, 20; in its description 8; in its body 2; an alias phrase of its front matter in the query
//! 20; the page among its `screens` 30.
//!
//! A match is **strong** (the tail's `skills that may fit`, and no strong false matches in the retrieval tests) when
//! the query is its name or title or holds it, when a phrase of two or more words is in its name, title or
//! description, when one of its front matter aliases (a word or a phrase) is in the query, or when its name words
//! matched are at least two or at least half of its name's words: one shared word with a three-word name (`orders` and
//! `place-sales-order`) is not.
//!
//! **The semantic branch** is fused by reciprocal rank (`1 / (60 + rank)`, as the search facade's hybrid search) when
//! there is an embedder: the search facade's (`search-facade semantic embedder`, which may be the LLM embedder), else
//! the LLM embedder of the `embed` feature when the `llm-facade` binds it ([`crate::llm::embed::bound_embedder`]): the
//! query against each skill's name, title, description, aliases and the text before its first section, embedded once
//! per content hash. A skill found only by meaning is never strong.

use std::collections::HashMap;
use std::sync::Mutex;

use super::{Skill, SkillIndex};
use crate::llm::tools::search::word_forms;
use crate::search::semantic::Embedder;

/// The reciprocal rank constant, as `crate::search::hybrid`.
const RRF_K: f64 = 60.0;
/// The semantic branch's candidates.
const SEMANTIC_TOP: usize = 20;

/// Intent and filler words: no loose token weight (Java's `QUERY_STOP`, and the words of a request).
pub const STOP_WORDS: &[&str] = &[
    "a", "about", "all", "an", "and", "any", "are", "as", "at", "be", "by", "can", "could", "do", "does", "find",
    "for", "from", "get", "give", "have", "help", "here", "how", "i", "if", "in", "into", "is", "it", "its", "let",
    "list", "me", "my", "need", "of", "on", "one", "open", "or", "our", "please", "should", "show", "some", "tell",
    "that", "the", "their", "them", "there", "these", "this", "those", "to", "up", "us", "use", "want", "we", "what",
    "when", "where", "which", "who", "why", "will", "with", "would", "you", "your",
];

/// The semantic branch's vectors, by embedder model, skill name and content hash.
pub type EmbeddingCache = Mutex<HashMap<(String, String, String), Vec<f32>>>;

/// One skill a query found.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub name: String,
    /// The order key: the lexical score, or the fused one when the semantic branch ran.
    pub score: f64,
    pub lexical: i64,
    /// The cosine of the semantic branch, when it ran and found the skill.
    pub semantic: Option<f32>,
    pub strong: bool,
    /// What matched, for RetrievalTest: `name:account`, `alias:login`, `phrase`, `page`, ….
    pub why: Vec<String>,
}

/// Lower case words.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// The words joined by one space.
fn phrase(text: &str) -> String {
    words(text).join(" ")
}

fn is_stop(word: &str) -> bool {
    STOP_WORDS.binary_search(&word).is_ok()
}

/// A query form matches a skill word: equal, or a prefix of it of four or more letters (`creat` of `create`).
fn form_hits(form: &str, word: &str) -> bool {
    word == form || (form.chars().count() >= 4 && word.starts_with(form))
}

fn any_hit(forms: &[String], words: &[String]) -> bool {
    forms.iter().any(|form| words.iter().any(|word| form_hits(form, word)))
}

/// A page as the `/apps/...` path the skills name: the `/qapps` and `/vapps` shells are the same screens.
fn page_path(page: &str) -> String {
    let path = page.split('?').next().unwrap_or(page).trim().trim_end_matches('/');
    for shell in ["/qapps/", "/vapps/"] {
        if let Some(rest) = path.strip_prefix(shell) {
            return format!("/apps/{rest}");
        }
    }
    path.to_string()
}

/// The lexical score of one skill.
pub fn score(index: &SkillIndex, skill: &Skill, text: &str, page: Option<&str>) -> Match {
    let mut found = Match {
        name: skill.name.clone(),
        score: 0.0,
        lexical: 0,
        semantic: None,
        strong: false,
        why: Vec::new(),
    };
    let query = phrase(text);
    let name_phrase = phrase(&skill.name);
    let title_phrase = phrase(&skill.title);
    let named: Vec<String> = words(&format!("{} {}", skill.name, skill.title));
    let name_content: Vec<String> = words(&skill.name).into_iter().filter(|word| !is_stop(word)).collect();
    let described: Vec<String> = words(&skill.description);
    let description_phrase = described.join(" ");
    let body: Vec<String> = words(&skill.body);
    // a one-word alias is a word of the skill; a longer one counts only as a whole phrase
    let alias_words: Vec<String> = skill.aliases.iter().filter(|alias| words(alias).len() == 1).map(|alias| phrase(alias)).collect();
    let mut content: Vec<String> = Vec::new();
    for word in words(text) {
        if !is_stop(&word) && !content.contains(&word) {
            content.push(word);
        }
    }
    let mut score: i64 = 0;
    let mut strong = false;
    if !query.is_empty() && (query == name_phrase || query == title_phrase) {
        score += 100;
        strong = true;
        found.why.push("name".to_string());
    } else if !content.is_empty()
        && query.chars().count() >= 3
        && (name_phrase.contains(&query) || title_phrase.contains(&query))
    {
        score += 40;
        strong = true;
        found.why.push("in-name".to_string());
    }
    if content.len() >= 2 {
        let joined = content.join(" ");
        if name_phrase.contains(&joined) || title_phrase.contains(&joined) || description_phrase.contains(&joined) {
            score += 40;
            strong = true;
            found.why.push("phrase".to_string());
        }
    }
    for alias in skill.aliases.iter().filter(|alias| words(alias).len() > 1) {
        let alias = phrase(alias);
        if !alias.is_empty() && format!(" {query} ").contains(&format!(" {alias} ")) {
            score += 20;
            strong = true;
            found.why.push(format!("alias:{alias}"));
        }
    }
    let mut named_hits = 0;
    for word in &content {
        let forms = word_forms(word);
        let widened: Vec<String> = index
            .alias_words(word)
            .iter()
            .flat_map(|alias| words(alias))
            .filter(|alias| !is_stop(alias))
            .collect();
        if any_hit(&forms, &named) {
            score += 20;
            named_hits += 1;
            found.why.push(format!("name:{word}"));
        } else if forms.iter().any(|form| alias_words.contains(form)) {
            // a front matter alias is the skill's own word for it: strong alone
            score += 20;
            named_hits += 1;
            strong = true;
            found.why.push(format!("alias:{word}"));
        } else if any_hit(&widened, &named) || widened.iter().any(|alias| alias_words.contains(alias)) {
            score += 10;
            named_hits += 1;
            found.why.push(format!("component-alias:{word}"));
        } else if any_hit(&forms, &described) {
            score += 8;
            found.why.push(format!("description:{word}"));
        } else if any_hit(&widened, &described) {
            score += 4;
            found.why.push(format!("description-alias:{word}"));
        } else if any_hit(&forms, &body) {
            score += 2;
            found.why.push(format!("body:{word}"));
        }
    }
    if named_hits >= 2 || (named_hits >= 1 && named_hits * 2 >= name_content.len().max(1)) {
        strong = true;
    }
    if let Some(page) = page.map(page_path).filter(|page| !page.is_empty()) {
        let on_page = skill.screens.iter().map(|screen| page_path(screen)).any(|screen| {
            !screen.is_empty() && (page == screen || page.starts_with(&format!("{screen}/")))
        });
        if on_page {
            score += 30;
            found.why.push("page".to_string());
        }
    }
    found.lexical = score;
    found.score = score as f64;
    found.strong = strong && score > 0;
    found
}

/// The skills that match `text` (and `page`), best first; ties by name. Lexical only.
pub fn rank(index: &SkillIndex, text: &str, page: Option<&str>) -> Vec<Match> {
    let mut out: Vec<Match> = index
        .skills
        .iter()
        .map(|skill| score(index, skill, text, page))
        .filter(|found| found.lexical > 0)
        .collect();
    out.sort_by(|left, right| right.lexical.cmp(&left.lexical).then_with(|| left.name.cmp(&right.name)));
    out
}

/// What the semantic branch embeds of a skill.
pub fn semantic_text(skill: &Skill) -> String {
    let (intro, _) = skill.sections();
    let intro: String = intro.chars().take(600).collect();
    format!("{}\n{}\n{}\n{}\n{}", skill.name.replace('-', " "), skill.title, skill.description, skill.aliases.join(", "), intro)
}

fn cosine(left: &[f32], right: &[f32]) -> f32 {
    let dot: f32 = left.iter().zip(right).map(|(a, b)| a * b).sum();
    let norm = |vector: &[f32]| vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    let denominator = norm(left) * norm(right);
    if denominator > 0.0 {
        dot / denominator
    } else {
        0.0
    }
}

/// [`rank`], with the semantic branch fused by reciprocal rank when `embedder` is given. `cache` keeps each skill's
/// vector by model, name and content hash. An embedder that fails is logged and the lexical ranking stands.
pub fn rank_hybrid(
    index: &SkillIndex,
    text: &str,
    page: Option<&str>,
    embedder: Option<&dyn Embedder>,
    cache: Option<&EmbeddingCache>,
) -> Vec<Match> {
    let lexical = rank(index, text, page);
    let Some(embedder) = embedder.filter(|_| !text.trim().is_empty() && !index.skills.is_empty()) else {
        return lexical;
    };
    let model = embedder.model().to_string();
    let key = |skill: &Skill| (model.clone(), skill.name.clone(), skill.content_hash.clone());
    let mut vectors: Vec<Option<Vec<f32>>> = index
        .skills
        .iter()
        .map(|skill| cache.and_then(|cache| cache.lock().ok()?.get(&key(skill)).cloned()))
        .collect();
    // the query first, with the embedder's short limit: a server that does not answer costs one short wait, then its
    // breaker skips it and skills rank by words
    let query = match embedder.embed_quick(&[text.to_string()]) {
        Ok(mut embedded) if embedded.len() == 1 => embedded.remove(0),
        Ok(embedded) => {
            tracing::error!("the embedder gave {} vectors for the skill query, so skills rank by words only", embedded.len());
            return lexical;
        }
        Err(error) => {
            tracing::error!("the skill query could not be embedded, so skills rank by words only: {error}");
            return lexical;
        }
    };
    // then the skills not embedded yet, a batch at a time, each batch kept as it comes; a batch that fails leaves its
    // skills to the words this time and the next query sends them again
    let missing: Vec<usize> = (0..vectors.len()).filter(|at| vectors[*at].is_none()).collect();
    for chunk in missing.chunks(crate::llm::embed::BATCH) {
        let texts: Vec<String> = chunk.iter().map(|at| semantic_text(&index.skills[*at])).collect();
        let embedded = match embedder.embed_quick(&texts) {
            Ok(embedded) if embedded.len() == texts.len() => embedded,
            Ok(embedded) => {
                tracing::error!("the embedder gave {} vectors for {} skill texts", embedded.len(), texts.len());
                break;
            }
            Err(error) => {
                tracing::error!("skills could not be embedded, so the rest rank by words only: {error}");
                break;
            }
        };
        for (at, vector) in chunk.iter().zip(embedded) {
            if let Some(cache) = cache {
                if let Ok(mut cache) = cache.lock() {
                    cache.insert(key(&index.skills[*at]), vector.clone());
                }
            }
            vectors[*at] = Some(vector);
        }
    }
    let mut semantic: Vec<(usize, f32)> = vectors
        .iter()
        .enumerate()
        .filter_map(|(at, vector)| vector.as_ref().map(|vector| (at, cosine(&query, vector))))
        .filter(|(_, similarity)| *similarity > 0.0)
        .collect();
    semantic.sort_by(|left, right| right.1.total_cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    semantic.truncate(SEMANTIC_TOP);
    let mut fused: Vec<Match> = lexical
        .iter()
        .enumerate()
        .map(|(rank, found)| Match {
            score: 1.0 / (RRF_K + (rank + 1) as f64),
            ..found.clone()
        })
        .collect();
    for (rank, (at, similarity)) in semantic.into_iter().enumerate() {
        let add = 1.0 / (RRF_K + (rank + 1) as f64);
        let name = &index.skills[at].name;
        match fused.iter_mut().find(|found| &found.name == name) {
            Some(found) => {
                found.score += add;
                found.semantic = Some(similarity);
            }
            None => fused.push(Match {
                name: name.clone(),
                score: add,
                lexical: 0,
                semantic: Some(similarity),
                strong: false,
                why: vec!["semantic".to_string()],
            }),
        }
    }
    fused.sort_by(|left, right| right.score.total_cmp(&left.score).then_with(|| left.name.cmp(&right.name)));
    fused
}

/// [`rank_hybrid`] with the embedder installed on the factory's search facade, else the LLM embedder of the `embed`
/// feature when it is bound, and the factory's cache.
pub fn rank_for(
    factory: &crate::context::ExecutionContextFactory,
    index: &SkillIndex,
    text: &str,
    page: Option<&str>,
) -> Vec<Match> {
    let embedder: Option<crate::search::semantic::SharedEmbedder> = crate::search::installed(factory)
        .and_then(|search| search.embedder())
        .or_else(|| {
            crate::llm::embed::bound_embedder(factory).map(|embedder| embedder as crate::search::semantic::SharedEmbedder)
        });
    let state = super::sync::state(factory);
    rank_hybrid(index, text, page, embedder.as_deref(), Some(&state.embeddings))
}

/// One case of a component's `test/skill-retrieval.json`.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct RetrievalCase {
    pub query: String,
    #[serde(default)]
    pub page: Option<String>,
    /// One of these must be in the top three; none means nothing may match strongly.
    #[serde(default)]
    pub expect: Vec<String>,
    /// These must not match strongly.
    #[serde(default)]
    pub negatives: Vec<String>,
}

#[derive(serde::Deserialize)]
struct RetrievalFile {
    cases: Vec<RetrievalCase>,
}

/// The cases of a `skill-retrieval.json`.
///
/// # Errors
///
/// Returns why the text is not a retrieval file.
pub fn read_cases(text: &str) -> Result<Vec<RetrievalCase>, String> {
    serde_json::from_str::<RetrievalFile>(text)
        .map(|file| file.cases)
        .map_err(|error| error.to_string())
}

/// What a retrieval set gave: recall@3 over the cases that expect a skill, and the strong false matches.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RetrievalReport {
    pub cases: usize,
    /// The cases that expect a skill, and how many had one in the top three.
    pub expected: usize,
    pub hits: usize,
    /// `query: got a, b, c` of each case that missed.
    pub misses: Vec<String>,
    /// `query: name` of each strong match a case forbids (a negative, or any for a case that expects none).
    pub strong_false: Vec<String>,
}

impl RetrievalReport {
    pub fn recall(&self) -> f64 {
        if self.expected == 0 {
            1.0
        } else {
            self.hits as f64 / self.expected as f64
        }
    }

    /// The summary line `moqui test --skills` prints.
    pub fn text(&self, label: &str) -> String {
        let mut lines = vec![format!(
            "Skill retrieval of {label}: recall@3 {:.2} ({} of {}), {} strong false matches, {} cases",
            self.recall(),
            self.hits,
            self.expected,
            self.strong_false.len(),
            self.cases
        )];
        lines.extend(self.misses.iter().map(|miss| format!("  missed  {miss}")));
        lines.extend(self.strong_false.iter().map(|false_match| format!("  strong  {false_match}")));
        lines.join("\n")
    }
}

/// Run `cases` over `index` with lexical retrieval (no model, no embedder).
pub fn evaluate(index: &SkillIndex, cases: &[RetrievalCase]) -> RetrievalReport {
    let mut report = RetrievalReport {
        cases: cases.len(),
        ..RetrievalReport::default()
    };
    for case in cases {
        let ranked = rank(index, &case.query, case.page.as_deref());
        if !case.expect.is_empty() {
            report.expected += 1;
            let top: Vec<&str> = ranked.iter().take(3).map(|found| found.name.as_str()).collect();
            if case.expect.iter().any(|name| top.contains(&name.as_str())) {
                report.hits += 1;
            } else {
                report.misses.push(format!("{}: got {}", case.query, top.join(", ")));
            }
        }
        for found in ranked.iter().filter(|found| found.strong) {
            let forbidden = case.negatives.contains(&found.name) || case.expect.is_empty();
            if forbidden {
                report.strong_false.push(format!("{}: {} ({})", case.query, found.name, found.why.join(", ")));
            }
        }
    }
    report
}
