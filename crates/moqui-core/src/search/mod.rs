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

//! `SearchFacade`: data documents searched and aggregated through one backend.
//!
//! Callers pass a [`Query`] tree and named [`Agg`] trees. A backend compiles them: Postgres SQL in
//! [`postgres`], or Elastic JSON on the OpenSearch backend. The design is `plans/archive/SearchFacade.md`.
//!
//! The facade is installed per factory by the host ([`install`]) and found from the entered
//! execution context ([`current`]).

pub mod agg;
pub mod conf;
pub mod es_json;
pub mod feed;
pub mod fresh;
pub mod highlight;
pub mod hybrid;
pub mod logs;
pub mod mapping;
#[cfg(feature = "http")]
pub mod opensearch;
pub mod parse;
pub mod postgres;
pub mod query;
pub mod semantic;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use indexmap::IndexMap;
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use moqui_lib::sync::{read, write};

pub use agg::Agg;
pub use mapping::{DocumentMapping, FieldKind, FieldMapping};
pub use query::{Query, TextMatch};

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::document::{self, Document};
use crate::entity::EntityFacade;

/// The deepest result window, as Elastic's `index.max_result_window`.
pub const MAX_WINDOW: usize = 10_000;

/// Control fields a document carries that are not stored in its body.
pub const CONTROL_FIELDS: [&str; 4] = ["_index", "_type", "_id", "_timestamp"];

/// A document type and the time its documents' data was read.
type ReadGroup = (String, Option<jiff::Timestamp>);

/// What a search runs over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// These `DataDocument` ids.
    Documents(Vec<String>),
    /// Every document whose `indexName` is this.
    Index(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SortField {
    /// A field name. A trailing `.keyword` names the unanalyzed value. `_score` and `_id` are the
    /// rank and the document id.
    pub field: String,
    pub descending: bool,
}

impl SortField {
    /// `-name` descending, `+name` or `name` ascending.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (descending, name) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };
        let name = name.trim();
        (!name.is_empty()).then(|| Self {
            field: name.to_string(),
            descending,
        })
    }
}

/// How text matches are ranked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rank {
    /// `ts_rank` over all the text, or OpenSearch's own score.
    #[default]
    Default,
    /// Cover density with the `searchWeight` field weights (`ts_rank_cd` on Postgres, field boosts
    /// on OpenSearch). Needs [`Capability::WeightedRank`].
    CoverDensity,
}

#[derive(Clone, Debug)]
pub struct SearchRequest {
    pub query: Query,
    pub sort: Vec<SortField>,
    pub offset: usize,
    pub limit: usize,
    /// Field names to return highlighted fragments for.
    pub highlight: Vec<String>,
    /// Read-your-writes: until this deadline, wait for this session's pending feed rows to be
    /// indexed and for a read replica to replay them; then run on the primary. `None` reads
    /// whatever the store has, which on a replica can be behind.
    pub fresh_by: Option<std::time::Instant>,
    pub rank: Rank,
    /// A trusted constraint the application adds, such as authorization: it must match too, it
    /// does not score or highlight, and it applies before ranking, paging, totals, and facets.
    pub filter: Option<Query>,
    /// How candidates are found: the query alone, or fused branches (`search::hybrid`).
    pub retrieval: Retrieval,
}

/// How a search finds its candidates.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Retrieval {
    /// The query, ranked by the backend. The total is exact.
    #[default]
    Lexical,
    /// Nearest neighbors of the text's embedding. Needs [`Capability::Semantic`]. The total is the
    /// candidate count, not the matching population.
    Semantic(hybrid::HybridOptions),
    /// Reciprocal rank fusion of the query, title prefix, fuzzy title, and semantic branches.
    /// Needs [`Capability::Hybrid`]. The total is exact over the non-semantic branches.
    Hybrid(hybrid::HybridOptions),
}

impl SearchRequest {
    /// The query and the trusted filter, both required.
    pub fn matching(&self) -> Query {
        match &self.filter {
            Some(filter) => Query::all(vec![self.query.clone(), filter.clone()]),
            None => self.query.clone(),
        }
    }
}

impl SearchRequest {
    pub fn new(query: Query) -> Self {
        Self {
            query,
            sort: Vec::new(),
            offset: 0,
            limit: 20,
            highlight: Vec::new(),
            fresh_by: None,
            rank: Rank::Default,
            filter: None,
            retrieval: Retrieval::Lexical,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    /// The `DataDocument` id, such as `MantleParty`.
    pub document_id: String,
    /// The record id, primary key values joined with `::`.
    pub id: String,
    /// The physical store name, `mantle_party` for `MantleParty`.
    pub index: String,
    pub score: f64,
    pub body: Document,
    pub highlights: IndexMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Default)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    /// Count of matching documents, not just this page; what it counts is `total_relation`.
    pub total: u64,
    pub total_relation: TotalRelation,
}

/// What [`SearchPage::total`] counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TotalRelation {
    /// Every document that matches. Lexical and hybrid searches.
    #[default]
    Eq,
    /// The bounded candidates a semantic search ranked.
    Candidates,
}

impl TotalRelation {
    pub fn name(self) -> &'static str {
        match self {
            Self::Eq => "eq",
            Self::Candidates => "candidates",
        }
    }
}

/// An opt-in search feature beyond the baseline query and aggregation trees. A backend lists the
/// ones it has; a request that needs one it lacks is an error naming it, never a quiet fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Capability {
    /// `Query::Near`: terms in order within a number of other words.
    Proximity,
    /// `Query::Fuzzy`: typo recovery on a document's title.
    Fuzzy,
    /// `Query::Suggest`: title and word prefixes as someone types.
    Autocomplete,
    /// `Rank::CoverDensity`: field weights from `searchWeight` and cover-density ranking.
    WeightedRank,
    /// Accent folding in the text configuration.
    Accent,
    /// Embedding retrieval.
    Semantic,
    /// Fusing lexical, fuzzy, and semantic candidates.
    Hybrid,
}

impl Capability {
    pub fn name(self) -> &'static str {
        match self {
            Self::Proximity => "proximity",
            Self::Fuzzy => "fuzzy",
            Self::Autocomplete => "autocomplete",
            Self::WeightedRank => "weightedRank",
            Self::Accent => "accent",
            Self::Semantic => "semantic",
            Self::Hybrid => "hybrid",
        }
    }
}

/// The error for a request that needs `capability` on a backend without it.
pub fn capability_error(capability: Capability, backend: &str) -> Error {
    Error::new(format!(
        "the {backend} search backend does not have the {} capability",
        capability.name()
    ))
}

/// A read replica's state, for `/metrics`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReplicaStatus {
    /// Seconds since the last replayed transaction, `None` when the replica cannot be read.
    pub lag_seconds: Option<f64>,
    /// Reads that ran on the primary: a fresh read that timed out, or a replica error.
    pub primary_fallbacks: u64,
}

/// One search store. The facade resolves targets and mappings; a backend compiles and runs.
pub trait SearchBackend: Send + Sync {
    fn name(&self) -> &'static str;
    /// Create the store for a document, or rebuild it when its stored shape changed.
    fn ensure_index(&self, mapping: &DocumentMapping) -> Result<(), Error>;
    fn index_exists(&self, mapping: &DocumentMapping) -> Result<bool, Error>;
    /// Insert or replace documents. Control fields are already removed from each body.
    fn index_documents(&self, mapping: &DocumentMapping, documents: &[(String, Document)]) -> Result<usize, Error>;
    /// Index documents whose data was read at `read_at`. A stored document read later is kept, so two deliveries
    /// of one record that finish out of order leave the newer read. Without `read_at` this is `index_documents`.
    fn index_documents_at(
        &self,
        mapping: &DocumentMapping,
        documents: &[(String, Document)],
        read_at: Option<jiff::Timestamp>,
    ) -> Result<usize, Error> {
        let _ = read_at;
        self.index_documents(mapping, documents)
    }
    fn delete_document(&self, mapping: &DocumentMapping, id: &str) -> Result<bool, Error>;
    fn delete_matching(&self, mapping: &DocumentMapping, filter: &Query) -> Result<u64, Error>;
    fn get_document(&self, mapping: &DocumentMapping, id: &str) -> Result<Option<Document>, Error>;
    /// `mappings` are the stores that exist for the target.
    fn search(&self, mappings: &[Arc<DocumentMapping>], request: &SearchRequest) -> Result<SearchPage, Error>;
    fn aggregate(
        &self,
        mapping: &DocumentMapping,
        filter: &Query,
        aggs: &IndexMap<String, Agg>,
    ) -> Result<IndexMap<String, Value>, Error>;
    fn drop_index(&self, mapping: &DocumentMapping) -> Result<(), Error>;
    /// [`Self::aggregate`] for read-your-writes, as [`SearchRequest::fresh_by`] describes. A backend
    /// with no read replica reads its one store.
    fn aggregate_fresh(
        &self,
        mapping: &DocumentMapping,
        filter: &Query,
        aggs: &IndexMap<String, Agg>,
        _fresh_by: Option<std::time::Instant>,
    ) -> Result<IndexMap<String, Value>, Error> {
        self.aggregate(mapping, filter, aggs)
    }
    /// Replica lag and how many reads fell back to the primary, when reads go to a replica.
    fn replica_status(&self) -> Option<ReplicaStatus> {
        None
    }
    /// The opt-in features this backend has.
    fn capabilities(&self) -> Vec<Capability> {
        Vec::new()
    }
    /// Nearest neighbors of `vector` among documents embedded with `model` that match `filter`,
    /// closest first, at most `k` over all of `mappings`. The score is cosine similarity.
    fn semantic_search(
        &self,
        _mappings: &[Arc<DocumentMapping>],
        _vector: &[f32],
        _model: &str,
        _filter: Option<&Query>,
        _k: usize,
    ) -> Result<Vec<SearchHit>, Error> {
        Err(capability_error(Capability::Semantic, self.name()))
    }
    /// Documents with no embedding of `model`, or one older than the stored body, at most `limit`.
    fn pending_embeddings(
        &self,
        _mapping: &DocumentMapping,
        _model: &str,
        _limit: usize,
    ) -> Result<Vec<semantic::PendingEmbedding>, Error> {
        Ok(Vec::new())
    }
    /// Store an embedding made from the body indexed at `seen`. False, and nothing stored, when
    /// the document was indexed again since, so a stale vector never replaces a newer body's.
    fn store_embedding(
        &self,
        _mapping: &DocumentMapping,
        _id: &str,
        _seen: &str,
        _model: &str,
        _vector: &[f32],
    ) -> Result<bool, Error> {
        Err(capability_error(Capability::Semantic, self.name()))
    }
    /// Record that `model` could not embed the body indexed at `seen`, so the document is not
    /// pending again until it is indexed again, and it has no vector. False when it was indexed
    /// again since.
    fn mark_embedding_failed(
        &self,
        _mapping: &DocumentMapping,
        _id: &str,
        _seen: &str,
        _model: &str,
    ) -> Result<bool, Error> {
        Err(capability_error(Capability::Semantic, self.name()))
    }
    /// Whether this backend keeps the log indexes (`search::logs`). OpenSearch and Postgres do.
    fn keeps_log_indexes(&self) -> bool {
        false
    }

    /// True when this backend is an Elastic-compatible server the `/search` and `/dashboard` proxies (`ProxyServlet.js`) can reach.
    fn has_elastic_api(&self) -> bool {
        false
    }
}

pub struct SearchFacade {
    backend: Arc<dyn SearchBackend>,
    entity: EntityFacade,
    mappings: Mutex<HashMap<String, Arc<DocumentMapping>>>,
    ensured: Mutex<HashSet<String>>,
    /// The data document definition generation `mappings` and `ensured` were filled under. A definition written here or
    /// on a cluster peer changes it, and the next use reads the definitions again.
    generation: AtomicU64,
    /// The local embedding model, when one is installed.
    embedder: RwLock<Option<semantic::SharedEmbedder>>,
}

impl SearchFacade {
    pub fn new(backend: Arc<dyn SearchBackend>, entity: EntityFacade) -> Self {
        Self {
            backend,
            entity,
            mappings: Mutex::new(HashMap::new()),
            ensured: Mutex::new(HashSet::new()),
            generation: AtomicU64::new(0),
            embedder: RwLock::new(None),
        }
    }

    /// Forget the mappings and the indexes ensured when a data document definition changed since they were made.
    fn follow_definitions(&self) {
        let now = crate::entity::cache::feed_definition_generation(&self.entity);
        if self.generation.swap(now, Ordering::AcqRel) != now {
            lock(&self.mappings).clear();
            lock(&self.ensured).clear();
        }
    }

    /// Install (or remove) the embedding model semantic search and the embedding worker use.
    pub fn set_embedder(&self, embedder: Option<semantic::SharedEmbedder>) {
        *write(&self.embedder) = embedder;
    }

    pub fn embedder(&self) -> Option<semantic::SharedEmbedder> {
        read(&self.embedder).clone()
    }

    /// Embed documents of `target` that have no current embedding, at most `batch` per document
    /// type. Returns how many were stored. Nothing when no embedder is installed or the backend
    /// has no semantic search.
    ///
    /// A document the model fails on, or gets a vector of the wrong length or with a non-finite
    /// value for, is logged and marked failed while others of its batch embed, so it does not
    /// stop the documents behind it. It is tried again when it is indexed again.
    ///
    /// # Errors
    ///
    /// Returns an error when the model fails on every document of a batch, or a store write fails.
    pub fn embed_pending(&self, target: &Target, batch: usize, max_chars: usize) -> Result<usize, Error> {
        let Some(embedder) = self.embedder() else {
            return Ok(0);
        };
        if !self.backend.capabilities().contains(&Capability::Semantic) {
            return Ok(0);
        }
        let mut stored = 0;
        for id in self.target_documents(target)? {
            let mapping = self.mapping(&id)?;
            if !self.backend.index_exists(&mapping)? {
                continue;
            }
            let pending = self.backend.pending_embeddings(&mapping, embedder.model(), batch)?;
            if pending.is_empty() {
                continue;
            }
            let texts: Vec<String> = pending
                .iter()
                .map(|item| semantic::embedding_text(&mapping, &item.body, max_chars))
                .collect();
            let results = embed_each(&*embedder, &texts)?;
            // No document of the batch embedded: the model is failing, not a document.
            if !results.iter().any(Result::is_ok) {
                let reason = results.into_iter().find_map(Result::err).unwrap_or_default();
                return Err(Error::new(format!(
                    "embedding {} failed: {reason}",
                    mapping.document_id
                )));
            }
            for (item, result) in pending.iter().zip(results) {
                match result {
                    Ok(vector) => {
                        if self
                            .backend
                            .store_embedding(&mapping, &item.id, &item.seen, embedder.model(), &vector)?
                        {
                            stored += 1;
                        }
                    }
                    Err(reason) => {
                        tracing::warn!(document = %mapping.document_id, id = %item.id, error = %reason,
                            "embedding failed; the document is skipped until it is indexed again");
                        self.backend
                            .mark_embedding_failed(&mapping, &item.id, &item.seen, embedder.model())?;
                    }
                }
            }
        }
        Ok(stored)
    }

    pub fn backend(&self) -> &Arc<dyn SearchBackend> {
        &self.backend
    }

    pub fn entity(&self) -> &EntityFacade {
        &self.entity
    }

    /// The stored shape of a document, from its `DataDocument` definition.
    ///
    /// # Errors
    ///
    /// Returns an error when the definition does not resolve.
    pub fn mapping(&self, data_document_id: &str) -> Result<Arc<DocumentMapping>, Error> {
        self.follow_definitions();
        if let Some(found) = lock(&self.mappings).get(data_document_id) {
            return Ok(Arc::clone(found));
        }
        if let Some(mapping) = self.log_mapping(data_document_id) {
            let mapping = Arc::new(mapping);
            lock(&self.mappings).insert(data_document_id.to_string(), Arc::clone(&mapping));
            return Ok(mapping);
        }
        let info = document::document_info(&self.entity, data_document_id)?;
        let mapping = Arc::new(DocumentMapping::from_info(&info));
        lock(&self.mappings).insert(data_document_id.to_string(), Arc::clone(&mapping));
        Ok(mapping)
    }

    /// Forget cached definitions and mappings, so the next call reads `DataDocument` again.
    pub fn clear_definitions(&self) {
        lock(&self.mappings).clear();
        lock(&self.ensured).clear();
        document::clear_document_cache(&self.entity);
    }

    /// Document ids for a target. An index with no documents is empty.
    ///
    /// # Errors
    ///
    /// Returns an error when the `DataDocument` table cannot be read.
    pub fn target_documents(&self, target: &Target) -> Result<Vec<String>, Error> {
        match target {
            Target::Documents(ids) => Ok(ids.clone()),
            Target::Index(name) if self.is_log_index(name) => Ok(vec![name.clone()]),
            Target::Index(name) => document::documents_in_index(&self.entity, name),
        }
    }

    /// The opt-in features available: the backend's, less semantic search when no embedder is
    /// installed.
    pub fn capabilities(&self) -> Vec<Capability> {
        let mut out = self.backend.capabilities();
        if self.embedder().is_none() {
            out.retain(|capability| *capability != Capability::Semantic);
        }
        out
    }

    /// An error naming the first of `needs` the backend does not have.
    fn require(&self, needs: &[Capability]) -> Result<(), Error> {
        if needs.is_empty() {
            return Ok(());
        }
        let have = self.capabilities();
        match needs.iter().find(|need| !have.contains(need)) {
            Some(missing) => Err(capability_error(*missing, self.backend.name())),
            None => Ok(()),
        }
    }

    /// True when `name` is a log index and this backend keeps the log indexes.
    pub fn is_log_index(&self, name: &str) -> bool {
        logs::is_log_index(name) && self.backend.keeps_log_indexes()
    }

    fn log_mapping(&self, name: &str) -> Option<DocumentMapping> {
        if self.backend.keeps_log_indexes() {
            logs::log_mapping(name)
        } else {
            None
        }
    }

    /// Index log entries (`search::logs`), each a flat body with its own `@timestamp`. The
    /// backend makes the id. Nothing is written when the backend does not keep log indexes.
    ///
    /// # Errors
    ///
    /// Returns an error when the index cannot be made or the bulk write fails.
    pub fn index_log_entries(&self, index: &str, entries: Vec<Document>) -> Result<usize, Error> {
        if !self.is_log_index(index) || entries.is_empty() {
            return Ok(0);
        }
        let mapping = self.ensure_cached(index)?;
        let documents: Vec<(String, Document)> = entries.into_iter().map(|body| (String::new(), body)).collect();
        self.backend.index_documents(&mapping, &documents)
    }

    /// Create the log indexes now, so the log viewer finds them before the first entry is written.
    /// Does nothing on a backend that keeps no log indexes.
    ///
    /// # Errors
    ///
    /// Returns an error when a store cannot be made.
    pub fn ensure_log_indexes(&self) -> Result<(), Error> {
        for index in [logs::APP_LOG, logs::HTTP_LOG] {
            if self.is_log_index(index) {
                self.ensure_cached(index)?;
            }
        }
        Ok(())
    }

    /// `ensure_index`: create or rebuild the store from the current definition.
    ///
    /// # Errors
    ///
    /// Returns an error when the definition does not resolve or the store cannot be made.
    pub fn ensure_index(&self, data_document_id: &str) -> Result<(), Error> {
        lock(&self.ensured).remove(data_document_id);
        lock(&self.mappings).remove(data_document_id);
        document::clear_document_cache(&self.entity);
        self.ensure_cached(data_document_id).map(|_| ())
    }

    fn ensure_cached(&self, data_document_id: &str) -> Result<Arc<DocumentMapping>, Error> {
        let mapping = self.mapping(data_document_id)?;
        if lock(&self.ensured).contains(data_document_id) {
            return Ok(mapping);
        }
        self.backend.ensure_index(&mapping)?;
        lock(&self.ensured).insert(data_document_id.to_string());
        Ok(mapping)
    }

    /// Index documents as `EntityDataDocument` builds them. Each names its document in `_type`
    /// and its record in `_id`. Control fields are not stored in the body.
    ///
    /// # Errors
    ///
    /// Returns an error when a document has no `_type` or `_id`, or the store write fails.
    pub fn index_documents(&self, documents: &[Document]) -> Result<usize, Error> {
        // By type, and by the time the document's data was read (`_timestamp`, set when a document is assembled).
        let mut grouped: IndexMap<ReadGroup, Vec<(String, Document)>> = IndexMap::new();
        for doc in documents {
            let doc_type = control_text(doc, "_type").ok_or_else(|| Error::new("a document to index has no _type"))?;
            let id = control_text(doc, "_id").ok_or_else(|| Error::new(format!("a {doc_type} document has no _id")))?;
            let read_at = control_text(doc, "_timestamp").and_then(|text| text.parse::<jiff::Timestamp>().ok());
            let mut body = doc.clone();
            for name in CONTROL_FIELDS {
                body.shift_remove(name);
            }
            grouped.entry((doc_type, read_at)).or_default().push((id, body));
        }
        let mut count = 0;
        for ((doc_type, read_at), docs) in grouped {
            let mapping = self.ensure_cached(&doc_type)?;
            count += self.backend.index_documents_at(&mapping, &docs, read_at)?;
        }
        Ok(count)
    }

    /// Remove one document. False when it was not there.
    ///
    /// # Errors
    ///
    /// Returns an error when the definition does not resolve or the store write fails.
    pub fn delete_document(&self, data_document_id: &str, id: &str) -> Result<bool, Error> {
        let mapping = self.mapping(data_document_id)?;
        if !self.backend.index_exists(&mapping)? {
            return Ok(false);
        }
        self.backend.delete_document(&mapping, id)
    }

    /// Remove every document of the target that matches `filter`. Returns how many.
    ///
    /// # Errors
    ///
    /// Returns an error when the filter does not compile or the store write fails.
    pub fn delete_matching(&self, target: &Target, filter: &Query) -> Result<u64, Error> {
        let mut count = 0;
        for id in self.target_documents(target)? {
            let mapping = self.mapping(&id)?;
            if self.backend.index_exists(&mapping)? {
                count += self.backend.delete_matching(&mapping, filter)?;
            }
        }
        Ok(count)
    }

    /// The stored body of one document, without control fields.
    ///
    /// # Errors
    ///
    /// Returns an error when the definition does not resolve or the read fails.
    pub fn get_document(&self, data_document_id: &str, id: &str) -> Result<Option<Document>, Error> {
        let mapping = self.mapping(data_document_id)?;
        if !self.backend.index_exists(&mapping)? {
            return Ok(None);
        }
        self.backend.get_document(&mapping, id)
    }

    /// True when at least one document of the target has a store.
    ///
    /// # Errors
    ///
    /// Returns an error when the definitions or the store cannot be read.
    pub fn target_exists(&self, target: &Target) -> Result<bool, Error> {
        for id in self.target_documents(target)? {
            if self.backend.index_exists(&*self.mapping(&id)?)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Search. Documents with no store yet are skipped, so a target with none is an empty page.
    ///
    /// # Errors
    ///
    /// Returns an error when the query does not compile against the documents, or the read fails.
    pub fn search(&self, target: &Target, request: &SearchRequest) -> Result<SearchPage, Error> {
        if request.limit > MAX_WINDOW || request.offset.saturating_add(request.limit) > MAX_WINDOW {
            return Err(Error::new(format!(
                "a search page must end within the first {MAX_WINDOW} results"
            )));
        }
        let mut mappings = Vec::new();
        for id in self.target_documents(target)? {
            let mapping = self.mapping(&id)?;
            if self.backend.index_exists(&mapping)? {
                mappings.push(mapping);
            }
        }
        if mappings.is_empty() {
            return Ok(SearchPage::default());
        }
        let mut needs = request.matching().capabilities();
        if request.rank == Rank::CoverDensity {
            needs.push(Capability::WeightedRank);
        }
        match &request.retrieval {
            Retrieval::Lexical => {}
            Retrieval::Hybrid(_) => needs.push(Capability::Hybrid),
            Retrieval::Semantic(_) => needs.push(Capability::Semantic),
        }
        self.require(&needs)?;
        if let Some(deadline) = request.fresh_by {
            fresh::wait_for_pending(deadline)?;
        }
        match &request.retrieval {
            Retrieval::Lexical => self.backend.search(&mappings, request),
            Retrieval::Hybrid(options) => {
                let embedder = self.embedder();
                hybrid::search(&*self.backend, &mappings, request, options, embedder.as_deref())
            }
            Retrieval::Semantic(options) => {
                let embedder = self
                    .embedder()
                    .ok_or_else(|| capability_error(Capability::Semantic, self.backend.name()))?;
                hybrid::semantic_search(&*self.backend, &mappings, request, options, &*embedder)
            }
        }
    }

    /// Aggregate over one document type. `filter` is the top-level constraint and does not score.
    ///
    /// # Errors
    ///
    /// Returns an error when the target is not one document, or a tree does not compile.
    pub fn aggregate(
        &self,
        target: &Target,
        filter: &Query,
        aggs: &IndexMap<String, Agg>,
    ) -> Result<IndexMap<String, Value>, Error> {
        self.aggregate_fresh(target, filter, aggs, None)
    }

    /// [`Self::aggregate`] with read-your-writes until `fresh_by`, as [`SearchRequest::fresh_by`].
    ///
    /// # Errors
    ///
    /// Returns an error when the target is not one document, or a tree does not compile.
    pub fn aggregate_fresh(
        &self,
        target: &Target,
        filter: &Query,
        aggs: &IndexMap<String, Agg>,
        fresh_by: Option<std::time::Instant>,
    ) -> Result<IndexMap<String, Value>, Error> {
        let ids = self.target_documents(target)?;
        if let Some(results) = self.document_facets(&ids, filter, aggs, fresh_by)? {
            return Ok(results);
        }
        let [id] = ids.as_slice() else {
            return Err(Error::new(format!(
                "aggregate runs over one document type, not {}; only terms on {} spans types",
                ids.len(),
                query::DOCUMENT_FIELD
            )));
        };
        let mapping = self.mapping(id)?;
        if !self.backend.index_exists(&mapping)? {
            return Ok(agg_empty_results(aggs));
        }
        self.require(&filter.capabilities())?;
        if let Some(deadline) = fresh_by {
            fresh::wait_for_pending(deadline)?;
        }
        self.backend.aggregate_fresh(&mapping, filter, aggs, fresh_by)
    }

    /// When every aggregation is a `terms` on `_document` with no sub-aggregations, the count of
    /// each document type matching `filter`, from exact search totals, so it works over any number
    /// of types on either backend. `None` for any other tree.
    fn document_facets(
        &self,
        ids: &[String],
        filter: &Query,
        aggs: &IndexMap<String, Agg>,
        fresh_by: Option<std::time::Instant>,
    ) -> Result<Option<IndexMap<String, Value>>, Error> {
        let facets: Vec<(&String, &agg::Terms)> = aggs
            .iter()
            .filter_map(|(name, agg)| match agg {
                Agg::Terms(terms) if terms.field == query::DOCUMENT_FIELD && terms.aggs.is_empty() => {
                    Some((name, terms))
                }
                _ => None,
            })
            .collect();
        if facets.is_empty() || facets.len() != aggs.len() {
            return Ok(None);
        }
        self.require(&filter.capabilities())?;
        if let Some(deadline) = fresh_by {
            fresh::wait_for_pending(deadline)?;
        }
        let mut counts: Vec<(String, u64)> = Vec::new();
        for id in ids {
            let mapping = self.mapping(id)?;
            if !self.backend.index_exists(&mapping)? {
                continue;
            }
            let mut request = SearchRequest::new(filter.clone());
            request.limit = 0;
            request.fresh_by = fresh_by;
            let total = self.backend.search(&[mapping], &request)?.total;
            if total > 0 {
                counts.push((id.clone(), total));
            }
        }
        let mut out = IndexMap::new();
        for (name, terms) in facets {
            let mut buckets = counts.clone();
            match &terms.order {
                agg::TermsOrder::Key { descending } => {
                    buckets.sort_by(|a, b| a.0.cmp(&b.0));
                    if *descending {
                        buckets.reverse();
                    }
                }
                agg::TermsOrder::Count { descending } => {
                    buckets.sort_by(|a, b| a.0.cmp(&b.0));
                    buckets.sort_by(|a, b| if *descending { b.1.cmp(&a.1) } else { a.1.cmp(&b.1) });
                }
                agg::TermsOrder::Metric { name, .. } => {
                    return Err(Error::new(format!(
                        "terms on {} has no metric {name} to order by",
                        query::DOCUMENT_FIELD
                    )));
                }
            }
            buckets.truncate(terms.size);
            let list = buckets
                .into_iter()
                .map(|(id, count)| {
                    let mut bucket = IndexMap::new();
                    bucket.insert("key".to_string(), Value::String(id.clone()));
                    bucket.insert("key_display".to_string(), Value::String(id));
                    bucket.insert(
                        "doc_count".to_string(),
                        Value::I64(i64::try_from(count).unwrap_or(i64::MAX)),
                    );
                    Value::Map(bucket)
                })
                .collect();
            let mut result = IndexMap::new();
            result.insert("buckets".to_string(), Value::List(list));
            out.insert(name.clone(), Value::Map(result));
        }
        Ok(Some(out))
    }

    /// Drop a document's store. The next index call makes it again.
    ///
    /// # Errors
    ///
    /// Returns an error when the definition does not resolve or the drop fails.
    pub fn drop_index(&self, data_document_id: &str) -> Result<(), Error> {
        let mapping = self.mapping(data_document_id)?;
        lock(&self.ensured).remove(data_document_id);
        self.backend.drop_index(&mapping)
    }
}

/// Results for a store that does not exist yet: empty buckets and zero metrics.
pub fn agg_empty_results(aggs: &IndexMap<String, Agg>) -> IndexMap<String, Value> {
    let mut out = IndexMap::new();
    for (name, agg) in aggs {
        out.insert(name.clone(), agg_empty(agg));
    }
    out
}

/// The result of one aggregation over no documents.
pub fn agg_empty(agg: &Agg) -> Value {
    match agg {
        Agg::Sum { .. } => Value::Decimal(Default::default()),
        Agg::Extreme { .. } => Value::Null,
        Agg::CountDistinct { .. } => Value::I64(0),
        Agg::Filter { aggs, .. } => {
            let mut map = agg_empty_results(aggs);
            map.insert("doc_count".to_string(), Value::I64(0));
            map.move_index(map.len() - 1, 0);
            Value::Map(map)
        }
        Agg::DateHistogram(_) | Agg::Terms(_) => {
            let mut map = IndexMap::new();
            map.insert("buckets".to_string(), Value::List(Vec::new()));
            Value::Map(map)
        }
    }
}

/// One vector or the reason there is none, for each text. A batch the model fails is embedded
/// again a text at a time, to find the texts it fails on, unless the embedder says its server is not
/// answering ([`semantic::Embedder::retries_each`]): then, and as soon as a text's retry says so, the
/// batch fails as a whole rather than costing a timeout a text. A vector of the wrong length or with
/// a non-finite value is a failure, since the store would refuse it.
///
/// # Errors
///
/// Returns the model's error for a batch of one text, or for a batch its server did not answer.
fn embed_each(embedder: &dyn semantic::Embedder, texts: &[String]) -> Result<Vec<Result<Vec<f32>, String>>, Error> {
    let vectors: Vec<Result<Vec<f32>, String>> = match embedder.embed(texts) {
        Ok(vectors) => vectors.into_iter().map(Ok).collect(),
        Err(err) if texts.len() <= 1 || !embedder.retries_each(&err) => return Err(err),
        Err(_) => {
            let mut each = Vec::with_capacity(texts.len());
            for text in texts {
                match embedder.embed(std::slice::from_ref(text)) {
                    Ok(mut one) => each.push(one.pop().ok_or_else(|| "the model returned no vector".to_string())),
                    Err(err) if !embedder.retries_each(&err) => return Err(err),
                    Err(err) => each.push(Err(err.message().to_string())),
                }
            }
            each
        }
    };
    let dimensions = embedder.dimensions();
    Ok(vectors
        .into_iter()
        .map(|vector| match vector {
            Ok(vector) if vector.len() != dimensions => {
                Err(format!("the vector has {} dimensions, not {dimensions}", vector.len()))
            }
            Ok(vector) if !vector.iter().all(|value| value.is_finite()) => {
                Err("the vector has a value that is not finite".to_string())
            }
            other => other,
        })
        .collect())
}

fn control_text(doc: &Document, name: &str) -> Option<String> {
    match doc.get(name) {
        None | Some(Value::Null) => None,
        Some(value) => Some(moqui_lib::object::to_plain_string(value)).filter(|text| !text.is_empty()),
    }
}

pub(crate) use moqui_lib::sync::lock;

#[derive(Default)]
struct Slot {
    facade: RwLock<Option<Arc<SearchFacade>>>,
}

/// Install the search facade on a factory. Feed receive and `ec.search` use it from then on.
pub fn install(factory: &ExecutionContextFactory, facade: SearchFacade) {
    let slot = factory.extension(|_| Slot::default());
    *write(&slot.facade) = Some(Arc::new(facade));
}

/// The facade installed on a factory.
pub fn installed(factory: &ExecutionContextFactory) -> Option<Arc<SearchFacade>> {
    let slot = factory.extension(|_| Slot::default());
    let found = read(&slot.facade).clone();
    found
}

/// The facade for the entered execution context.
///
/// # Errors
///
/// Returns an error when no context is entered or no search backend is configured.
pub fn current() -> Result<Arc<SearchFacade>, Error> {
    ExecutionContext::with_current(|ec| installed(ec.factory()))
        .ok_or_else(|| Error::new("no execution context is entered"))?
        .ok_or_else(|| Error::new("search is not configured"))
}

/// Java's `ElasticFacadeImpl` start, run by each factory: every DataFeed with `indexOnStartEmpty` Y that has a
/// document with no store yet gets an `IndexDataFeedDocuments` job for the feed. Runs in the entered context, with
/// authorization disabled as Java's finds. Nothing happens when search is not installed. A failure is logged, as Java
/// does, and does not stop the start.
pub fn index_feeds_on_start_empty() {
    let Ok(search) = current() else {
        return;
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let outcome = (|| -> Result<(), Error> {
        let entity = search.entity();
        let feeds = entity
            .find("moqui.entity.feed.DataFeed")?
            .eq("indexOnStartEmpty", Value::string("Y"))
            .list()?;
        for feed in feeds.values() {
            let Some(feed_id) = feed.get("dataFeedId").map(moqui_lib::object::to_plain_string) else {
                continue;
            };
            let details = entity
                .find("moqui.entity.feed.DataFeedDocumentDetail")?
                .eq("dataFeedId", Value::string(&feed_id))
                .list()?;
            let mut missing = false;
            for detail in details.values() {
                let Some(document_id) = detail.get("dataDocumentId").map(moqui_lib::object::to_plain_string) else {
                    continue;
                };
                if !search.backend.index_exists(&*search.mapping(&document_id)?)? {
                    missing = true;
                }
            }
            if missing {
                let mut parameters = std::collections::BTreeMap::new();
                parameters.insert("dataFeedId".to_string(), Value::string(&feed_id));
                let job_run_id = crate::service::run_job_now("IndexDataFeedDocuments", &parameters)?;
                tracing::info!("Found index does not exist for DataFeed {feed_id}, started job {job_run_id} to index");
            }
        }
        Ok(())
    })();
    if let Err(err) = outcome {
        tracing::error!("Error checking or indexing for all DataFeed with indexOnStartEmpty=Y: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_fields_parse_signs() {
        assert_eq!(
            SortField::parse("-userAccounts.username.keyword"),
            Some(SortField {
                field: "userAccounts.username.keyword".into(),
                descending: true
            })
        );
        assert_eq!(SortField::parse("+pseudoId").map(|sort| sort.descending), Some(false));
        assert_eq!(SortField::parse("  "), None);
    }

    #[test]
    fn empty_results_have_zero_metrics_and_no_buckets() {
        let aggs = Agg::map_from_value(&moqui_lib::json::from_json(
            &serde_json::from_str(
                r#"{"total":{"sum":{"field":"x"}},"P":{"filter":{"term":{"t":"A"}},"aggs":{"n":{"cardinality":{"field":"o"}}}},
                    "D":{"date_histogram":{"field":"d","unit":"day"}}}"#,
            )
            .unwrap(),
        ))
        .unwrap();
        let empty = agg_empty_results(&aggs);
        assert!(matches!(empty["total"], Value::Decimal(_)));
        let Value::Map(filter) = &empty["P"] else { panic!() };
        assert_eq!(filter.keys().next().map(String::as_str), Some("doc_count"));
        assert_eq!(filter["n"], Value::I64(0));
    }

    /// A semantic store holding the given pending bodies, recording what is stored and marked.
    struct EmbedStore {
        pending: Vec<&'static str>,
        stored: Mutex<Vec<String>>,
        failed: Mutex<Vec<String>>,
    }

    impl SearchBackend for EmbedStore {
        fn name(&self) -> &'static str {
            "embed-test"
        }
        fn keeps_log_indexes(&self) -> bool {
            true
        }
        fn capabilities(&self) -> Vec<Capability> {
            vec![Capability::Semantic]
        }
        fn ensure_index(&self, _mapping: &DocumentMapping) -> Result<(), Error> {
            Ok(())
        }
        fn index_exists(&self, _mapping: &DocumentMapping) -> Result<bool, Error> {
            Ok(true)
        }
        fn index_documents(
            &self,
            _mapping: &DocumentMapping,
            _documents: &[(String, Document)],
        ) -> Result<usize, Error> {
            Ok(0)
        }
        fn delete_document(&self, _mapping: &DocumentMapping, _id: &str) -> Result<bool, Error> {
            Ok(false)
        }
        fn delete_matching(&self, _mapping: &DocumentMapping, _filter: &Query) -> Result<u64, Error> {
            Ok(0)
        }
        fn get_document(&self, _mapping: &DocumentMapping, _id: &str) -> Result<Option<Document>, Error> {
            Ok(None)
        }
        fn search(&self, _mappings: &[Arc<DocumentMapping>], _request: &SearchRequest) -> Result<SearchPage, Error> {
            Ok(SearchPage::default())
        }
        fn aggregate(
            &self,
            _mapping: &DocumentMapping,
            _filter: &Query,
            _aggs: &IndexMap<String, Agg>,
        ) -> Result<IndexMap<String, Value>, Error> {
            Ok(IndexMap::new())
        }
        fn drop_index(&self, _mapping: &DocumentMapping) -> Result<(), Error> {
            Ok(())
        }
        fn pending_embeddings(
            &self,
            _mapping: &DocumentMapping,
            _model: &str,
            _limit: usize,
        ) -> Result<Vec<semantic::PendingEmbedding>, Error> {
            Ok(self
                .pending
                .iter()
                .map(|text| semantic::PendingEmbedding {
                    id: (*text).to_string(),
                    seen: "t1".to_string(),
                    body: Document::from([("message".to_string(), Value::string(*text))]),
                })
                .collect())
        }
        fn store_embedding(
            &self,
            _mapping: &DocumentMapping,
            id: &str,
            _seen: &str,
            _model: &str,
            _vector: &[f32],
        ) -> Result<bool, Error> {
            lock(&self.stored).push(id.to_string());
            Ok(true)
        }
        fn mark_embedding_failed(
            &self,
            _mapping: &DocumentMapping,
            id: &str,
            _seen: &str,
            _model: &str,
        ) -> Result<bool, Error> {
            lock(&self.failed).push(id.to_string());
            Ok(true)
        }
    }

    /// Fails a batch with `poison` in it, and gives `nan` and `short` bad vectors.
    struct PickyModel;

    impl semantic::Embedder for PickyModel {
        fn model(&self) -> &str {
            "picky"
        }
        fn dimensions(&self) -> usize {
            2
        }
        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, Error> {
            if texts.iter().any(|text| text.contains("poison")) {
                return Err(Error::new("the model cannot read poison"));
            }
            Ok(texts
                .iter()
                .map(|text| {
                    if text.starts_with("nan") {
                        vec![f32::NAN, 0.0]
                    } else if text.starts_with("short") {
                        vec![1.0]
                    } else {
                        vec![1.0, 0.0]
                    }
                })
                .collect())
        }
    }

    fn embed_with(pending: Vec<&'static str>) -> (Result<usize, Error>, Vec<String>, Vec<String>) {
        let store = Arc::new(EmbedStore {
            pending,
            stored: Mutex::new(Vec::new()),
            failed: Mutex::new(Vec::new()),
        });
        let facade = SearchFacade::new(Arc::clone(&store) as Arc<dyn SearchBackend>, crate::testing::open());
        facade.set_embedder(Some(Arc::new(PickyModel)));
        let result = facade.embed_pending(&Target::Documents(vec![logs::APP_LOG.to_string()]), 10, 100);
        let stored = lock(&store.stored).clone();
        let failed = lock(&store.failed).clone();
        (result, stored, failed)
    }

    /// A data document definition written here or on a cluster peer (which counts in the same generation) makes the
    /// search facade read its mappings again.
    #[test]
    fn a_definition_write_refreshes_the_cached_mappings() {
        let store = Arc::new(EmbedStore {
            pending: Vec::new(),
            stored: Mutex::new(Vec::new()),
            failed: Mutex::new(Vec::new()),
        });
        let entity = crate::testing::open();
        let facade = SearchFacade::new(Arc::clone(&store) as Arc<dyn SearchBackend>, entity.clone());
        let first = facade.mapping(logs::APP_LOG).unwrap();
        assert!(Arc::ptr_eq(&first, &facade.mapping(logs::APP_LOG).unwrap()));
        let mut document = entity.make_value("moqui.entity.document.DataDocument").unwrap();
        document.set("dataDocumentId", Value::string("REFRESHED"));
        document.set("primaryEntityName", Value::string("moqui.basic.Enumeration"));
        document.create(&entity).unwrap();
        assert!(!Arc::ptr_eq(&first, &facade.mapping(logs::APP_LOG).unwrap()));
    }

    #[test]
    fn a_document_the_model_fails_on_is_marked_and_the_rest_embed() {
        let (result, stored, failed) = embed_with(vec!["first", "poison", "nan", "short", "last"]);
        assert_eq!(result.unwrap(), 2);
        assert_eq!(stored, ["first", "last"]);
        assert_eq!(failed, ["poison", "nan", "short"]);

        // A model that fails on everything is the model's error, and nothing is marked.
        let (result, stored, failed) = embed_with(vec!["poison one", "poison two"]);
        assert!(result.unwrap_err().message().contains("cannot read poison"));
        assert!(stored.is_empty() && failed.is_empty());
    }
}
