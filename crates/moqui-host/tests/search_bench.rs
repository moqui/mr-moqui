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

//! Search benchmarks over a generated catalog corpus: indexing throughput and store size (the maintenance cost),
//! single-document write-to-visible latency (freshness, without the feed's scheduling), and p50/p95 latency for
//! broad and selective lexical queries, phrase, proximity, fuzzy, autocomplete, hybrid, and a facet aggregation.
//!
//! Ignored by default. Run it in release mode, with the corpus size in `SEARCH_BENCH_DOCS` (default 50000):
//!
//! ```text
//! SEARCH_BENCH_DOCS=100000 cargo test --release -p moqui-host --test search_bench -- --ignored --nocapture
//! ```
//!
//! With `OPENSEARCH_URL` set it also runs against that OpenSearch. Results go in `plans/Performance.md`.

#![cfg(all(unix, feature = "embedded-postgres"))]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;
use moqui_core::driver::DbDriver;
use moqui_core::entity::document::Document;
use moqui_core::search::agg::Agg;
use moqui_core::search::hybrid::{self, HybridOptions};
use moqui_core::search::mapping::{DisplayFields, DocumentMapping, FieldKind, FieldMapping, ObjectMapping};
use moqui_core::search::postgres::PostgresBackend;
use moqui_core::search::{Query, Rank, SearchBackend, SearchRequest};
use moqui_core::transaction::{PoolConfig, TransactionManager};
use moqui_host::db::embedded::EmbeddedCluster;
use moqui_lib::object::Value;

#[test]
#[ignore = "a benchmark: SEARCH_BENCH_DOCS=50000 cargo test --release -p moqui-host --test search_bench -- --ignored --nocapture"]
fn search_benchmarks() {
    let docs: usize = std::env::var("SEARCH_BENCH_DOCS")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(50_000);
    let corpus = Corpus::new(docs);

    let cluster_dir = temp_dir("msb");
    let _guard = DirGuard(cluster_dir.clone());
    std::fs::create_dir_all(&cluster_dir).unwrap();
    let (cluster, driver) = EmbeddedCluster::start(&cluster_dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let tx = TransactionManager::new(Arc::clone(&driver), pool()).unwrap();
    let backend = PostgresBackend::open(tx, None).unwrap();
    run(&backend, &corpus, Some(&*driver));
    drop(backend);
    drop(cluster);

    if let Ok(url) = std::env::var("OPENSEARCH_URL") {
        let conf = moqui_core::search::conf::ClusterConf {
            url,
            user: std::env::var("OPENSEARCH_USER").ok(),
            password: std::env::var("OPENSEARCH_PASSWORD").ok(),
            index_prefix: Some(format!("moqui_search_bench_{}_", std::process::id())),
        };
        let backend = moqui_core::search::opensearch::OpenSearchBackend::new(&conf);
        run(&backend, &corpus, None);
    }
}

/// A named search, and the request for each round.
type Case<'a> = (&'a str, Box<dyn Fn(usize) -> SearchRequest + 'a>);

fn run(backend: &dyn SearchBackend, corpus: &Corpus, driver: Option<&dyn DbDriver>) {
    let mapping = mapping();
    let _ = backend.drop_index(&mapping);
    backend.ensure_index(&mapping).unwrap();
    println!("\n== {} with {} documents", backend.name(), corpus.docs.len());

    // Indexing: batches of 500, as the feed and a rebuild deliver them.
    let started = Instant::now();
    for batch in corpus.docs.chunks(500) {
        backend.index_documents(&mapping, batch).unwrap();
    }
    let elapsed = started.elapsed();
    println!(
        "index: {:.1}s, {:.0} documents/s",
        elapsed.as_secs_f64(),
        corpus.docs.len() as f64 / elapsed.as_secs_f64()
    );
    if let Some(driver) = driver {
        let mut conn = driver.connect().unwrap();
        let rows = conn
            .query(
                "SELECT pg_total_relation_size('search.bench_catalog')::bigint AS bytes",
                &[],
            )
            .unwrap();
        let bytes: f64 = rows[0]
            .get("bytes")
            .map(moqui_lib::object::to_plain_string)
            .and_then(|text| text.parse().ok())
            .unwrap_or(0.0);
        println!(
            "store: {:.1} MB with indexes, {:.2} KB per document",
            bytes / 1_048_576.0,
            bytes / 1024.0 / corpus.docs.len() as f64
        );
    }

    // Freshness: one document written, then searched for by its code, until found.
    let mut visible = Vec::new();
    for round in 0..50 {
        let code = format!("FRESH{round:04}");
        let doc = item(&code, &format!("fresh item {round}"), "fresh", "Freshness", 0.5);
        let started = Instant::now();
        backend.index_documents(&mapping, &[doc]).unwrap();
        loop {
            let page = backend
                .search(&[Arc::new(mapping.clone())], &SearchRequest::new(term("code", &code)))
                .unwrap();
            if page.total > 0 {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "{code} never became visible"
            );
        }
        visible.push(started.elapsed());
    }
    report("write to visible", &mut visible);

    let mappings = vec![Arc::new(mapping.clone())];
    let common = &corpus.vocabulary[..10];
    let rare = &corpus.vocabulary[corpus.vocabulary.len() - 200..];
    let rounds = 50;
    let pick = |words: &[String], round: usize| words[round % words.len()].clone();
    let lexical = |text: String| query(&format!(r#"{{"text": {{"terms": "{text}"}}}}"#));
    let cases: Vec<Case<'_>> = vec![
        (
            "broad lexical (a common word)",
            Box::new(|round| SearchRequest::new(lexical(pick(common, round)))),
        ),
        (
            "selective lexical (a rare word)",
            Box::new(|round| SearchRequest::new(lexical(pick(rare, round)))),
        ),
        (
            "two words, cover density rank",
            Box::new(|round| {
                let mut request =
                    SearchRequest::new(lexical(format!("{} {}", pick(common, round), pick(common, round + 3))));
                request.rank = Rank::CoverDensity;
                request
            }),
        ),
        (
            "phrase (two title words)",
            Box::new(|round| {
                let title = &corpus.titles[round * 97 % corpus.titles.len()];
                let words: Vec<&str> = title.split(' ').take(2).collect();
                SearchRequest::new(query(&format!(
                    r#"{{"text": {{"terms": "{}", "match": "phrase"}}}}"#,
                    words.join(" ")
                )))
            }),
        ),
        (
            "near (two words within 3)",
            Box::new(|round| {
                SearchRequest::new(query(&format!(
                    r#"{{"near": {{"terms": "{} {}", "distance": 3}}}}"#,
                    pick(common, round),
                    pick(common, round + 1)
                )))
            }),
        ),
        (
            "fuzzy (a typo in a title word)",
            Box::new(|round| {
                let title = &corpus.titles[round * 31 % corpus.titles.len()];
                let word = title.split(' ').next().unwrap();
                SearchRequest::new(query(&format!(r#"{{"fuzzy": {{"terms": "{}"}}}}"#, typo(word))))
            }),
        ),
        (
            "autocomplete (three letters)",
            Box::new(|round| {
                let title = &corpus.titles[round * 53 % corpus.titles.len()];
                SearchRequest::new(query(&format!(
                    r#"{{"suggest": {{"text": "{}"}}}}"#,
                    &title[..3.min(title.len())]
                )))
            }),
        ),
    ];
    for (name, make) in &cases {
        let mut times = Vec::new();
        let mut hits = 0;
        for round in 0..rounds {
            let request = make(round);
            let started = Instant::now();
            let page = backend.search(&mappings, &request).unwrap();
            times.push(started.elapsed());
            hits += page.total;
        }
        report(&format!("{name}, {} average total", hits / rounds as u64), &mut times);
    }

    let mut times = Vec::new();
    for round in 0..rounds {
        let options = HybridOptions {
            text: format!("{} {}", pick(common, round), pick(rare, round)),
            ..HybridOptions::default()
        };
        let started = Instant::now();
        hybrid::search(backend, &mappings, &SearchRequest::new(Query::MatchAll), &options, None, &options.text).unwrap();
        times.push(started.elapsed());
    }
    report("hybrid (a common and a rare word)", &mut times);

    let aggs: IndexMap<String, Agg> =
        Agg::map_from_value(&json(r#"{"categories": {"terms": {"field": "category", "size": 50}}}"#)).unwrap();
    let mut times = Vec::new();
    for round in 0..rounds {
        let filter = lexical(pick(common, round));
        let started = Instant::now();
        backend.aggregate(&mapping, &filter, &aggs).unwrap();
        times.push(started.elapsed());
    }
    report("category facet over a broad query", &mut times);

    backend.drop_index(&mapping).unwrap();
}

/// The generated corpus: titles of two to four words, descriptions of twenty to forty, from a vocabulary whose
/// word frequencies fall off as in text (Zipf), and a category of twenty.
struct Corpus {
    vocabulary: Vec<String>,
    titles: Vec<String>,
    docs: Vec<(String, Document)>,
}

impl Corpus {
    fn new(count: usize) -> Self {
        let mut random = Random(0x2545_f491_4f6c_dd1d);
        let vocabulary: Vec<String> = (0..4000).map(|index| word(&mut random, index)).collect();
        // Zipf over the vocabulary: the cumulative weights of 1/rank.
        let mut cumulative = Vec::with_capacity(vocabulary.len());
        let mut sum = 0.0;
        for rank in 1..=vocabulary.len() {
            sum += 1.0 / rank as f64;
            cumulative.push(sum);
        }
        let draw = |random: &mut Random| {
            let target = random.unit() * sum;
            let index = cumulative
                .partition_point(|weight| *weight < target)
                .min(vocabulary.len() - 1);
            vocabulary[index].clone()
        };
        let categories: Vec<String> = (0..20).map(|index| format!("Category {index:02}")).collect();
        let mut titles = Vec::with_capacity(count);
        let mut docs = Vec::with_capacity(count);
        for index in 0..count {
            let title_words = 2 + random.below(3);
            let title: Vec<String> = (0..title_words).map(|_| capitalize(&draw(&mut random))).collect();
            let title = title.join(" ");
            let description_words = 20 + random.below(21);
            let description: Vec<String> = (0..description_words).map(|_| draw(&mut random)).collect();
            let category = &categories[random.below(categories.len())];
            let quality = random.unit();
            titles.push(title.clone());
            docs.push(item(
                &format!("ITEM{index:07}"),
                &title,
                &description.join(" "),
                category,
                quality,
            ));
        }
        Corpus {
            vocabulary,
            titles,
            docs,
        }
    }
}

fn item(code: &str, title: &str, description: &str, category: &str, quality: f64) -> (String, Document) {
    let doc: Document = [
        ("code", Value::string(code)),
        ("title", Value::string(title)),
        ("description", Value::string(description)),
        ("category", Value::string(category)),
        ("quality", Value::F64(quality)),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value))
    .collect();
    (code.to_string(), doc)
}

fn mapping() -> DocumentMapping {
    let field = |kind| FieldMapping { kind, sortable: false };
    let mut root = ObjectMapping::default();
    root.fields.insert("code".into(), field(FieldKind::Keyword));
    root.fields.insert("title".into(), field(FieldKind::Text));
    root.fields.insert("description".into(), field(FieldKind::Text));
    root.fields.insert("category".into(), field(FieldKind::Keyword));
    root.fields.insert("quality".into(), field(FieldKind::Double));
    let mut display = DisplayFields::default();
    display.roles.insert("title".into(), "title".into());
    display.roles.insert("code".into(), "code".into());
    display.roles.insert("category".into(), "category".into());
    display.roles.insert("quality".into(), "quality".into());
    display.weights.insert("title".into(), 'A');
    display.weights.insert("description".into(), 'C');
    DocumentMapping {
        document_id: "BenchCatalog".into(),
        index_name: None,
        physical_name: "bench_catalog".into(),
        root,
        manual_mapping_service: None,
        display,
    }
}

/// A pronounceable word, unique by index.
fn word(random: &mut Random, index: usize) -> String {
    const CONSONANTS: &[u8] = b"bcdfghklmnprstvz";
    const VOWELS: &[u8] = b"aeiou";
    let syllables = 2 + random.below(2);
    let mut text = String::new();
    for _ in 0..syllables {
        text.push(CONSONANTS[random.below(CONSONANTS.len())] as char);
        text.push(VOWELS[random.below(VOWELS.len())] as char);
    }
    format!("{text}{}", base26(index))
}

fn base26(mut index: usize) -> String {
    let mut text = String::new();
    loop {
        text.insert(0, (b'a' + (index % 26) as u8) as char);
        index /= 26;
        if index == 0 {
            return text;
        }
    }
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// One letter changed in the middle, lower-cased.
fn typo(word: &str) -> String {
    let mut chars: Vec<char> = word.to_lowercase().chars().collect();
    let middle = chars.len() / 2;
    chars[middle] = if chars[middle] == 'x' { 'y' } else { 'x' };
    chars.into_iter().collect()
}

fn term(field: &str, value: &str) -> Query {
    query(&format!(
        r#"{{"field": {{"name": "{field}", "op": "eq", "value": "{value}"}}}}"#
    ))
}

fn query(text: &str) -> Query {
    Query::from_value(&json(text)).unwrap()
}

fn json(text: &str) -> Value {
    moqui_lib::json::from_json(&serde_json::from_str(text).unwrap())
}

fn report(name: &str, times: &mut [Duration]) {
    times.sort();
    let at = |fraction: f64| {
        times[((times.len() as f64 * fraction).ceil() as usize)
            .saturating_sub(1)
            .min(times.len() - 1)]
    };
    let ms = |duration: Duration| duration.as_secs_f64() * 1000.0;
    println!(
        "{name}: p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        ms(at(0.5)),
        ms(at(0.95)),
        ms(*times.last().unwrap())
    );
}

/// xorshift64: the corpus is the same on every run.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1_u64 << 53) as f64
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

fn pool() -> PoolConfig {
    PoolConfig {
        min: 0,
        max: 8,
        reserve: 2,
        acquire_timeout: Duration::from_secs(30),
        ..Default::default()
    }
}

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    PathBuf::from("/tmp").join(format!("{prefix}-{}-{nanos}", std::process::id()))
}

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
