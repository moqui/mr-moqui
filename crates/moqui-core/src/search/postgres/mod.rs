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

//! The Postgres search backend.
//!
//! One table per data document in the `search` schema: `doc_id` (the record id), `indexed_at`,
//! `body` (the nested document as `jsonb`), a typed column `f_<name>` for each root field, and a
//! generated `all_text` vector over every string and number in the body. Child arrays stay in
//! `body`. `search.document_index` records each table and the signature of the shape it was made
//! with, so a definition change rebuilds the table.
//!
//! DDL runs on a reserve connection, autocommit. Reads run on the caller's connection, inside its
//! transaction when one is open. Writes join the caller's transaction or begin their own.

pub mod agg;
pub mod compile;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use jiff::Timestamp;
use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

use compile::{
    column, column_type, ident, literal, typed_param, Compiler, Located, Sql, TextSetup, NORM_TITLE, RANKED_TEXT,
};

use super::highlight::highlights;
use super::mapping::{DocumentMapping, FieldKind};
use super::query::Query;
use super::{lock, Agg, Capability, Rank, ReplicaStatus, SearchBackend, SearchHit, SearchPage, SearchRequest};
use crate::driver::{DbConnection, DbRow};
use crate::entity::document::Document;
use crate::transaction::{TransactionAttribute, TransactionManager};

pub const DEFAULT_SCHEMA: &str = "search";
/// How long a missing table is believed without looking again.
const ABSENT_TTL: Duration = Duration::from_secs(10);

pub struct PostgresBackend {
    tx: TransactionManager,
    /// A physical replica of the store for searches, aggregations, and `get_document`.
    read: Option<TransactionManager>,
    /// Reads that ran on the primary because a fresh read timed out or the replica failed.
    fallbacks: AtomicU64,
    schema: String,
    trigram: bool,
    /// The text configuration and title fold the tables are made with.
    text: TextSetup,
    /// True when `text` folds accents (`unaccent`).
    accent: bool,
    /// Embedding storage, when semantic search is enabled and `pgvector` is installed.
    vectors: Option<VectorStore>,
    /// Signature of each table known to exist.
    known: Mutex<HashMap<String, String>>,
    /// When each document id was last found to have no table. Trusted for [`ABSENT_TTL`], since
    /// another node may make the table.
    absent: Mutex<HashMap<String, Instant>>,
    /// One `ensure_index` at a time in this process; the advisory lock covers other processes.
    ensuring: Mutex<()>,
}

/// How embeddings are stored: `halfvec` or `vector`, of `dimensions`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorStore {
    pub representation: String,
    pub dimensions: usize,
}

impl VectorStore {
    fn column_type(&self) -> String {
        format!("{}({})", self.representation, self.dimensions)
    }
}

/// `[0.1,0.2,..]`, the text form `pgvector` reads.
pub fn vector_literal(vector: &[f32]) -> String {
    let parts: Vec<String> = vector.iter().map(|value| format!("{value}")).collect();
    format!("[{}]", parts.join(","))
}

/// A nearest-neighbor request: the query vector, the model its embeddings come from, a filter, and how many hits.
pub struct SemanticQuery<'a> {
    pub vector: &'a [f32],
    pub model: &'a str,
    pub filter: Option<&'a Query>,
    pub k: usize,
}

/// The nearest-neighbor statement: per document type, the closest `k` embedded with `model` that
/// match `filter`, cosine distance on the HNSW index, then the closest `k` of those.
///
/// # Errors
///
/// Returns an error when the filter does not compile against a document.
pub fn semantic_sql(
    schema: &str,
    text: &TextSetup,
    store: &VectorStore,
    mappings: &[Arc<DocumentMapping>],
    query: &SemanticQuery<'_>,
) -> Result<Sql, Error> {
    let &SemanticQuery {
        vector,
        model,
        filter,
        k,
    } = query;
    let cast = store.column_type();
    let literal_vector = vector_literal(vector);
    let mut parts = Vec::new();
    for mapping in mappings {
        let compiler = Compiler::with_text(mapping, text);
        let table = format!("{}.{}", ident(schema), ident(&mapping.physical_name));
        let mut part = Sql::new(format!(
            "(SELECT {} AS document_id, {} AS index_name, doc_id, body::text AS body, (1 - (embedding <=> ",
            literal(&mapping.document_id),
            literal(&mapping.physical_name)
        ));
        part.param(Value::string(&literal_vector)).push(&format!(
            "::{cast}))::float8 AS score FROM {table} WHERE embedding IS NOT NULL AND embedding_model = "
        ));
        part.param(Value::string(model));
        if let Some(filter) = filter {
            part.push(" AND ");
            part.append(compiler.where_sql(filter)?);
        }
        part.push(" ORDER BY embedding <=> ");
        part.param(Value::string(&literal_vector))
            .push(&format!("::{cast} LIMIT {k})"));
        parts.push(part);
    }
    let mut out = Sql::new("SELECT * FROM (");
    for (index, part) in parts.into_iter().enumerate() {
        if index > 0 {
            out.push(" UNION ALL ");
        }
        out.append(part);
    }
    out.push(&format!(
        ") AS s ORDER BY score DESC, document_id ASC, doc_id ASC LIMIT {k}"
    ));
    Ok(out)
}

/// Where one read runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    Primary,
    Replica,
}

/// How often a fresh read asks the replica how far it has replayed.
const REPLAY_POLL: Duration = Duration::from_millis(20);

impl PostgresBackend {
    /// Open the store: create the schema, `pg_trgm` when it can, and the registry table.
    ///
    /// # Errors
    ///
    /// Returns an error when the schema or the registry cannot be created.
    pub fn open(tx: TransactionManager, schema: Option<&str>) -> Result<Self, Error> {
        let schema = schema.unwrap_or(DEFAULT_SCHEMA).to_string();
        let quoted = ident(&schema);
        let trigram = tx.with_reserve(|conn| {
            conn.execute(&format!("CREATE SCHEMA IF NOT EXISTS {quoted}"), &[])?;
            let trigram = match conn.execute("CREATE EXTENSION IF NOT EXISTS pg_trgm", &[]) {
                Ok(_) => true,
                Err(err) => {
                    tracing::warn!(error = %err, "pg_trgm is not available; contains searches will scan");
                    false
                }
            };
            conn.execute(
                &format!(
                    "CREATE TABLE IF NOT EXISTS {quoted}.document_index (document_id text PRIMARY KEY, \
                     index_name text, physical_name text NOT NULL, signature text NOT NULL, \
                     created_at timestamptz NOT NULL DEFAULT now())"
                ),
                &[],
            )?;
            Ok(trigram)
        })?;
        Ok(Self {
            tx,
            read: None,
            fallbacks: AtomicU64::new(0),
            schema,
            trigram,
            text: TextSetup::default(),
            accent: false,
            vectors: None,
            known: Mutex::new(HashMap::new()),
            absent: Mutex::new(HashMap::new()),
            ensuring: Mutex::new(()),
        })
    }

    /// Store embeddings when `conf` enables semantic search and the `pgvector` extension can be
    /// created; otherwise a warning, and no semantic capability. Enabling it rebuilds the tables.
    ///
    /// # Errors
    ///
    /// Returns an error when the conf is enabled with zero dimensions.
    pub fn with_semantic(mut self, conf: &crate::search::semantic::SemanticConf) -> Result<Self, Error> {
        if !conf.enabled {
            return Ok(self);
        }
        if conf.dimensions == 0 {
            return Err(Error::new("semantic search needs dimensions above 0"));
        }
        let created = self
            .tx
            .with_reserve(|conn| conn.execute("CREATE EXTENSION IF NOT EXISTS vector", &[]));
        match created {
            Ok(_) => {
                self.vectors = Some(VectorStore {
                    representation: conf.representation.clone(),
                    dimensions: conf.dimensions,
                });
            }
            Err(err) => {
                tracing::warn!(error = %err, "pgvector is not available; semantic search is off");
            }
        }
        Ok(self)
    }

    /// Use the text search configuration `config` (default `simple`), folding accents when
    /// `unaccent` is set: a copy of the configuration with `unaccent` before its word dictionaries,
    /// and a lower-and-unaccent function for titles, both in the store's schema. When the
    /// `unaccent` extension cannot be created, accents are not folded and a warning says so. A
    /// change of either rebuilds every table on its next `ensure_index`.
    ///
    /// # Errors
    ///
    /// Returns an error when `config` does not exist or the configuration cannot be made.
    pub fn with_analysis(mut self, config: Option<&str>, unaccent: bool) -> Result<Self, Error> {
        let base = config.unwrap_or(compile::TS_CONFIG).to_string();
        let quoted = ident(&self.schema);
        let schema = self.schema.clone();
        let setup = self.tx.with_reserve(|conn| {
            let found = conn.query("SELECT 1 FROM pg_ts_config WHERE cfgname = ?", &[Value::string(&base)])?;
            if found.is_empty() {
                return Err(Error::new(format!("text search configuration {base} does not exist")));
            }
            if !unaccent {
                return Ok((
                    TextSetup {
                        config: base.clone(),
                        fold: "lower".to_string(),
                    },
                    false,
                ));
            }
            if let Err(err) = conn.execute("CREATE EXTENSION IF NOT EXISTS unaccent", &[]) {
                tracing::warn!(error = %err, "unaccent is not available; search does not fold accents");
                return Ok((
                    TextSetup {
                        config: base.clone(),
                        fold: "lower".to_string(),
                    },
                    false,
                ));
            }
            let rows = conn.query(
                "SELECT n.nspname AS schema FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace \
                 WHERE e.extname = 'unaccent'",
                &[],
            )?;
            let ext_schema = rows
                .first()
                .and_then(|row| row.get("schema"))
                .map(to_plain_string)
                .unwrap_or_else(|| "public".to_string());
            let ext = ident(&ext_schema);
            let name = format!("moqui_unaccent_{}", base.replace(|ch: char| !ch.is_ascii_alphanumeric(), "_"));
            let exists = conn.query(
                "SELECT 1 FROM pg_ts_config c JOIN pg_namespace n ON n.oid = c.cfgnamespace \
                 WHERE c.cfgname = ? AND n.nspname = ?",
                &[Value::string(&name), Value::string(&schema)],
            )?;
            if exists.is_empty() {
                // The dictionaries the base configuration gives words, after unaccent.
                let dicts = conn.query(
                    "SELECT d.dictname AS dict FROM pg_ts_config c JOIN pg_ts_config_map m ON m.mapcfg = c.oid \
                     JOIN pg_ts_dict d ON d.oid = m.mapdict \
                     WHERE c.cfgname = ? AND m.maptokentype = (SELECT tokid FROM ts_token_type('default') WHERE alias = 'word') \
                     ORDER BY m.mapseqno",
                    &[Value::string(&base)],
                )?;
                let dicts: Vec<String> = dicts
                    .iter()
                    .filter_map(|row| row.get("dict").map(to_plain_string))
                    .collect();
                let dicts = if dicts.is_empty() { vec!["simple".to_string()] } else { dicts };
                conn.execute(
                    &format!("CREATE TEXT SEARCH CONFIGURATION {quoted}.{} (COPY = {})", ident(&name), ident(&base)),
                    &[],
                )?;
                conn.execute(
                    &format!(
                        "ALTER TEXT SEARCH CONFIGURATION {quoted}.{} ALTER MAPPING FOR hword, hword_part, word WITH {ext}.unaccent, {}",
                        ident(&name),
                        dicts.iter().map(|dict| ident(dict)).collect::<Vec<_>>().join(", ")
                    ),
                    &[],
                )?;
            }
            // unaccent() is only stable; a generated column needs an immutable function.
            conn.execute(
                &format!(
                    "CREATE OR REPLACE FUNCTION {quoted}.moqui_fold(text) RETURNS text LANGUAGE sql IMMUTABLE PARALLEL SAFE \
                     AS $$ SELECT lower({ext}.unaccent({}::regdictionary, $1)) $$",
                    literal(&format!("{ext_schema}.unaccent"))
                ),
                &[],
            )?;
            Ok((
                TextSetup {
                    config: format!("{schema}.{name}"),
                    fold: format!("{quoted}.moqui_fold"),
                },
                true,
            ))
        })?;
        self.text = setup.0;
        self.accent = setup.1;
        Ok(self)
    }

    /// The registry signature: the stored shape and everything a table derives from text setup.
    fn signature(&self, mapping: &DocumentMapping) -> String {
        let text = format!(
            "{}|config={}|fold={}|trigram={}|vectors={}",
            mapping.signature(),
            self.text.config,
            self.text.fold,
            self.trigram,
            self.vectors.as_ref().map(VectorStore::column_type).unwrap_or_default()
        );
        format!("{:016x}", moqui_lib::codec::fnv1a(text.as_bytes()))
    }

    /// Run searches, aggregations, and `get_document` on `read`, a physical replica of this store
    /// (`read-url`). Index and schema writes, the registry, and reads inside a transaction on the
    /// primary stay on the primary.
    #[must_use]
    pub fn with_read_replica(mut self, read: TransactionManager) -> Self {
        self.read = Some(read);
        self
    }

    /// Where a read runs. With no replica, or inside a transaction on the primary, the primary.
    /// Otherwise the replica; for read-your-writes (`fresh_by`) only once it has replayed the
    /// primary's WAL position as of now, which covers each commit the caller already waited for.
    /// A read that cannot be fresh by the deadline runs on the primary.
    fn read_route(&self, fresh_by: Option<Instant>) -> Route {
        let Some(read) = &self.read else {
            return Route::Primary;
        };
        if self.tx.in_transaction() {
            return Route::Primary;
        }
        let Some(deadline) = fresh_by else {
            return Route::Replica;
        };
        let lsn = match self
            .tx
            .with_connection(|conn| conn.query("SELECT pg_current_wal_lsn()::text AS lsn", &[]))
        {
            Ok(rows) => rows.first().and_then(|row| row.get("lsn")).map(to_plain_string),
            Err(err) => {
                tracing::warn!(error = %err, "could not read the primary WAL position; the read runs on the primary");
                None
            }
        };
        let Some(lsn) = lsn else {
            self.fallbacks.fetch_add(1, Ordering::Relaxed);
            return Route::Primary;
        };
        // A read-url that is not in recovery has no replay position; it is as current as it gets.
        let check = "SELECT COALESCE(pg_last_wal_replay_lsn() >= ?::pg_lsn, true) AS caught_up";
        loop {
            match read.with_connection(|conn| conn.query(check, &[Value::string(&lsn)])) {
                Ok(rows) if rows.first().and_then(|row| row.get("caught_up")) == Some(&Value::Bool(true)) => {
                    return Route::Replica;
                }
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!(error = %err, "the search replica cannot be read; the read runs on the primary");
                    break;
                }
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(REPLAY_POLL);
        }
        self.fallbacks.fetch_add(1, Ordering::Relaxed);
        Route::Primary
    }

    /// Run a read where `route` says. A replica error, such as a statement cancelled for replay,
    /// runs it again on the primary.
    fn query_on(&self, route: Route, sql: &Sql) -> Result<Vec<DbRow>, Error> {
        match (route, &self.read) {
            (Route::Replica, Some(read)) => {
                tracing::debug!(sql = %sql.text, "search query on the replica");
                match bounded_query(read, sql) {
                    Ok(rows) => Ok(rows),
                    Err(err) => {
                        tracing::warn!(error = %err, "search query failed on the replica; running it on the primary");
                        self.fallbacks.fetch_add(1, Ordering::Relaxed);
                        self.query(sql)
                    }
                }
            }
            _ => self.query(sql),
        }
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }

    fn table(&self, mapping: &DocumentMapping) -> String {
        format!("{}.{}", ident(&self.schema), ident(&mapping.physical_name))
    }

    fn query(&self, sql: &Sql) -> Result<Vec<DbRow>, Error> {
        tracing::debug!(sql = %sql.text, "search query");
        bounded_query(&self.tx, sql)
    }

    /// The signature the table of `document_id` was made with, `None` when there is no table. A
    /// missing table is remembered for [`ABSENT_TTL`], so a search over a type that was never
    /// indexed does not take a reserve connection each time.
    fn registered(&self, document_id: &str) -> Result<Option<String>, Error> {
        if let Some(signature) = lock(&self.known).get(document_id) {
            return Ok(Some(signature.clone()));
        }
        if lock(&self.absent)
            .get(document_id)
            .is_some_and(|at| at.elapsed() < ABSENT_TTL)
        {
            return Ok(None);
        }
        let mut sql = Sql::new(format!(
            "SELECT signature FROM {}.document_index WHERE document_id = ",
            ident(&self.schema)
        ));
        sql.param(Value::string(document_id));
        let rows = self.tx.with_reserve(|conn| conn.query(&sql.text, &sql.params))?;
        let found = rows.first().and_then(|row| row.get("signature")).map(to_plain_string);
        match &found {
            Some(signature) => {
                lock(&self.absent).remove(document_id);
                lock(&self.known).insert(document_id.to_string(), signature.clone());
            }
            None => {
                lock(&self.absent).insert(document_id.to_string(), Instant::now());
            }
        }
        Ok(found)
    }

    fn create_statements(&self, mapping: &DocumentMapping) -> Vec<String> {
        let table = self.table(mapping);
        let config = literal(&self.text.config);
        let mut columns = vec![
            "doc_id text PRIMARY KEY".to_string(),
            "indexed_at timestamptz NOT NULL".to_string(),
            "body jsonb NOT NULL".to_string(),
            format!(
                "all_text tsvector GENERATED ALWAYS AS (jsonb_to_tsvector({config}, body, '[\"string\",\"numeric\"]')) STORED"
            ),
        ];
        let mut indexes = vec![
            format!("CREATE INDEX ON {table} USING gin (all_text)"),
            format!("CREATE INDEX ON {table} (indexed_at)"),
        ];
        for (name, field) in &mapping.root.fields {
            let Some(sql_type) = column_type(field.kind) else {
                continue;
            };
            let col = column(name);
            columns.push(format!("{col} {sql_type}"));
            match field.kind {
                FieldKind::Text => {
                    indexes.push(format!(
                        "CREATE INDEX ON {table} USING gin (to_tsvector({config}, {col}))"
                    ));
                    if self.trigram {
                        indexes.push(format!("CREATE INDEX ON {table} USING gin ({col} gin_trgm_ops)"));
                    }
                }
                FieldKind::Keyword | FieldKind::Date | FieldKind::Long | FieldKind::Double => {
                    indexes.push(format!("CREATE INDEX ON {table} ({col})"));
                }
                FieldKind::Boolean | FieldKind::Binary => {}
            }
        }
        let compiler = Compiler::with_text(mapping, &self.text);
        if let Some(title) = compiler.title_field() {
            // The folded title for fuzzy (trigram nearest neighbor) and autocomplete (prefix).
            columns.push(format!(
                "{NORM_TITLE} text GENERATED ALWAYS AS ({}({})) STORED",
                self.text.fold,
                column(title)
            ));
            indexes.push(format!("CREATE INDEX ON {table} ({NORM_TITLE} text_pattern_ops)"));
            if self.trigram {
                indexes.push(format!(
                    "CREATE INDEX ON {table} USING gist ({NORM_TITLE} gist_trgm_ops)"
                ));
            }
        }
        if let Some(store) = &self.vectors {
            // A partial HNSW index: documents not yet embedded are not in it.
            columns.push(format!("embedding {}", store.column_type()));
            columns.push("embedding_model text".to_string());
            columns.push("embedding_indexed_at timestamptz".to_string());
            indexes.push(format!(
                "CREATE INDEX ON {table} USING hnsw (embedding {}_cosine_ops) WHERE embedding IS NOT NULL",
                store.representation
            ));
        }
        if !mapping.display.weights.is_empty() {
            // Weighted fields at their weight, then all the text at D, for cover-density ranking.
            let mut parts: Vec<String> = mapping
                .display
                .weights
                .iter()
                .filter(|(name, _)| mapping.root.fields.contains_key(*name))
                .map(|(name, weight)| {
                    format!(
                        "setweight(to_tsvector({config}, coalesce({}, '')), '{weight}')",
                        column(name)
                    )
                })
                .collect();
            parts.push(format!(
                "setweight(jsonb_to_tsvector({config}, body, '[\"string\",\"numeric\"]'), 'D')"
            ));
            columns.push(format!(
                "{RANKED_TEXT} tsvector GENERATED ALWAYS AS ({}) STORED",
                parts.join(" || ")
            ));
        }
        let mut out = vec![format!("CREATE TABLE {table} ({})", columns.join(", "))];
        out.extend(indexes);
        out
    }
}

fn bounded_query(tx: &TransactionManager, sql: &Sql) -> Result<Vec<DbRow>, Error> {
    match super::limits::remaining()? {
        None => tx.with_connection(|connection| connection.query(&sql.text, &sql.params)),
        Some(timeout) => tx.run(TransactionAttribute::ForceNew, Some(timeout), || {
            tx.with_connection(|connection| {
                connection.execute(
                    &format!("SET LOCAL statement_timeout = '{}'", timeout.as_millis().max(1)),
                    &[],
                )?;
                connection.query(&sql.text, &sql.params)
            })
        }),
    }
}

/// Bind parameters in one statement. The protocol counts them in 16 bits, and the client takes no more than `i16::MAX`.
const MAX_INSERT_PARAMS: usize = 32_767;
/// Documents in one `INSERT`, below the parameter limit.
const MAX_INSERT_ROWS: usize = 1000;

/// A typed column of a document table: the root field it holds, its kind, and the cast its insert parameter takes.
struct InsertColumn<'a> {
    name: &'a str,
    kind: FieldKind,
    cast: &'static str,
}

/// The typed columns of a mapping's table, in table order.
fn insert_columns(mapping: &DocumentMapping) -> Vec<InsertColumn<'_>> {
    mapping
        .root
        .fields
        .iter()
        .filter(|(_, field)| column_type(field.kind).is_some())
        .map(|(name, field)| InsertColumn {
            name,
            kind: field.kind,
            cast: match field.kind {
                FieldKind::Long => "::bigint",
                FieldKind::Double => "::numeric",
                FieldKind::Date => "::timestamptz",
                FieldKind::Boolean => "::boolean",
                _ => "",
            },
        })
        .collect()
}

/// One `INSERT ... ON CONFLICT` of `rows` documents. With `if_newer`, a stored row read later than the one given is kept.
fn insert_sql(table: &str, columns: &[InsertColumn<'_>], rows: usize, if_newer: bool) -> String {
    let mut statement = format!("INSERT INTO {table} (doc_id, indexed_at, body");
    for item in columns {
        statement.push_str(", ");
        statement.push_str(&column(item.name));
    }
    let mut row = String::from("(?, ?::timestamptz, ?::jsonb");
    for item in columns {
        row.push_str(", ?");
        row.push_str(item.cast);
    }
    row.push(')');
    statement.push_str(") VALUES ");
    for index in 0..rows {
        if index > 0 {
            statement.push_str(", ");
        }
        statement.push_str(&row);
    }
    statement.push_str(" ON CONFLICT (doc_id) DO UPDATE SET indexed_at = EXCLUDED.indexed_at, body = EXCLUDED.body");
    for item in columns {
        let col = column(item.name);
        statement.push_str(&format!(", {col} = EXCLUDED.{col}"));
    }
    if if_newer {
        statement.push_str(&format!(" WHERE {table}.indexed_at <= EXCLUDED.indexed_at"));
    }
    statement
}

/// The parameters of one document's row, in the order of [`insert_sql`].
fn push_insert_params(
    mapping: &DocumentMapping,
    columns: &[InsertColumn<'_>],
    id: &str,
    at: Timestamp,
    body: &Document,
    out: &mut Vec<Value>,
) -> Result<(), Error> {
    // A date in a child element is stored only in the body, which the queries cast to timestamptz.
    let mut body = body.clone();
    mapping.root.type_dates(&mut body);
    let body = &body;
    let json = serde_json::to_string(&body_json_map(body))
        .map_err(|err| Error::new(format!("document {id} is not JSON: {err}")))?;
    out.extend([Value::string(id), Value::Timestamp(at), Value::String(json)]);
    for item in columns {
        let value = match body.get(item.name) {
            None | Some(Value::Null) | Some(Value::List(_)) | Some(Value::Map(_)) => None,
            Some(value) => typed_param(item.kind, value),
        };
        out.push(match value {
            // A long column takes a whole number; numeric text like 3.0 would not cast.
            Some((Value::Decimal(decimal), _)) if item.cast == "::bigint" => {
                Value::String(decimal.trunc().normalize().to_string())
            }
            Some((value, _)) => value,
            None => Value::Null,
        });
    }
    Ok(())
}

/// 13 hex digits of epoch millis then 12 random ones, so rows sort by arrival and ids do not collide.
pub(crate) fn generated_doc_id() -> String {
    let mut bytes = [0_u8; 6];
    if getrandom::fill(&mut bytes).is_err() {
        bytes = u64::from(std::process::id()).to_le_bytes()[..6]
            .try_into()
            .unwrap_or([0; 6]);
    }
    let mut id = format!("{:013x}", Timestamp::now().as_millisecond());
    for byte in bytes {
        id.push_str(&format!("{byte:02x}"));
    }
    id
}

impl SearchBackend for PostgresBackend {
    fn upsert_counter(
        &self,
        mapping: &DocumentMapping,
        id: &str,
        document: &Document,
        counter: &str,
    ) -> Result<(), Error> {
        let table = self.table(mapping);
        let columns = insert_columns(mapping);
        let mut params = Vec::new();
        push_insert_params(mapping, &columns, id, Timestamp::now(), document, &mut params)?;
        let mut statement = insert_sql(&table, &columns, 1, false);
        let counter_column = column(counter);
        let key = literal(counter);
        let increment = format!("COALESCE({table}.{counter_column},0) + COALESCE(EXCLUDED.{counter_column},0)");
        let body = format!("jsonb_set(EXCLUDED.body, ARRAY[{key}], to_jsonb({increment}))");
        let body = if mapping.root.fields.contains_key("first_seen") {
            format!("jsonb_set(jsonb_set({body}, ARRAY['first_seen'], to_jsonb(LEAST({table}.{}, EXCLUDED.{}))), ARRAY['last_seen'], to_jsonb(GREATEST({table}.{}, EXCLUDED.{})))",column("first_seen"),column("first_seen"),column("last_seen"),column("last_seen"))
        } else {
            body
        };
        statement = statement.replace("body = EXCLUDED.body", &format!("body = {body}"));
        statement = statement.replace(
            &format!("{counter_column} = EXCLUDED.{counter_column}"),
            &format!("{counter_column} = {increment}"),
        );
        if mapping.root.fields.contains_key("first_seen") {
            let col = column("first_seen");
            statement = statement.replace(
                &format!("{col} = EXCLUDED.{col}"),
                &format!("{col} = LEAST({table}.{col}, EXCLUDED.{col})"),
            );
            let col = column("last_seen");
            statement = statement.replace(
                &format!("{col} = EXCLUDED.{col}"),
                &format!("{col} = GREATEST({table}.{col}, EXCLUDED.{col})"),
            );
        }
        self.tx.run(TransactionAttribute::UseOrBegin, None, || {
            self.tx
                .with_connection(|connection| connection.execute(&statement, &params))
        })?;
        Ok(())
    }
    fn name(&self) -> &'static str {
        "postgres"
    }

    fn capabilities(&self) -> Vec<Capability> {
        let mut out = vec![
            Capability::Proximity,
            Capability::Autocomplete,
            Capability::WeightedRank,
            Capability::Hybrid,
        ];
        if self.trigram {
            out.push(Capability::Fuzzy);
        }
        if self.accent {
            out.push(Capability::Accent);
        }
        if self.vectors.is_some() {
            out.push(Capability::Semantic);
        }
        out
    }

    fn semantic_search(
        &self,
        mappings: &[Arc<DocumentMapping>],
        vector: &[f32],
        model: &str,
        filter: Option<&Query>,
        k: usize,
    ) -> Result<Vec<SearchHit>, Error> {
        let store = self
            .vectors
            .as_ref()
            .ok_or_else(|| super::capability_error(Capability::Semantic, "postgres"))?;
        let query = SemanticQuery {
            vector,
            model,
            filter,
            k,
        };
        let sql = semantic_sql(&self.schema, &self.text, store, mappings, &query)?;
        let rows = self.query_on(self.read_route(None), &sql)?;
        rows.iter()
            .map(|row| {
                Ok(SearchHit {
                    document_id: row.get("document_id").map(to_plain_string).unwrap_or_default(),
                    id: row.get("doc_id").map(to_plain_string).unwrap_or_default(),
                    index: row.get("index_name").map(to_plain_string).unwrap_or_default(),
                    score: row
                        .get("score")
                        .map(|value| to_plain_string(value).parse::<f64>().unwrap_or(0.0))
                        .unwrap_or(0.0),
                    body: parse_body(row.get("body"))?,
                    highlights: IndexMap::new(),
                })
            })
            .collect()
    }

    fn pending_embeddings(
        &self,
        mapping: &DocumentMapping,
        model: &str,
        limit: usize,
    ) -> Result<Vec<crate::search::semantic::PendingEmbedding>, Error> {
        if self.vectors.is_none() {
            return Ok(Vec::new());
        }
        // A row marked failed has the model and the index time and no vector: not pending until it is indexed again.
        let mut sql = Sql::new(format!(
            "SELECT doc_id, indexed_at::text AS seen, body::text AS body FROM {} \
             WHERE embedding_model IS DISTINCT FROM ",
            self.table(mapping)
        ));
        sql.param(Value::string(model)).push(&format!(
            " OR embedding_indexed_at IS NULL OR embedding_indexed_at < indexed_at ORDER BY indexed_at LIMIT {limit}"
        ));
        let rows = self.query(&sql)?;
        rows.iter()
            .map(|row| {
                Ok(crate::search::semantic::PendingEmbedding {
                    id: row.get("doc_id").map(to_plain_string).unwrap_or_default(),
                    seen: row.get("seen").map(to_plain_string).unwrap_or_default(),
                    body: parse_body(row.get("body"))?,
                })
            })
            .collect()
    }

    fn store_embedding(
        &self,
        mapping: &DocumentMapping,
        id: &str,
        seen: &str,
        model: &str,
        vector: &[f32],
    ) -> Result<bool, Error> {
        let store = self
            .vectors
            .as_ref()
            .ok_or_else(|| super::capability_error(Capability::Semantic, "postgres"))?;
        if vector.len() != store.dimensions {
            return Err(Error::new(format!(
                "an embedding of {} dimensions does not fit the search store's {}",
                vector.len(),
                store.dimensions
            )));
        }
        let statement = format!(
            "UPDATE {} SET embedding = ?::{}, embedding_model = ?, embedding_indexed_at = indexed_at \
             WHERE doc_id = ? AND indexed_at = ?::timestamptz",
            self.table(mapping),
            store.column_type()
        );
        let params = [
            Value::String(vector_literal(vector)),
            Value::string(model),
            Value::string(id),
            Value::string(seen),
        ];
        let count = self.tx.run(TransactionAttribute::UseOrBegin, None, || {
            self.tx.with_connection(|conn| conn.execute(&statement, &params))
        })?;
        Ok(count > 0)
    }

    fn mark_embedding_failed(
        &self,
        mapping: &DocumentMapping,
        id: &str,
        seen: &str,
        model: &str,
    ) -> Result<bool, Error> {
        if self.vectors.is_none() {
            return Err(super::capability_error(Capability::Semantic, "postgres"));
        }
        let statement = format!(
            "UPDATE {} SET embedding = NULL, embedding_model = ?, embedding_indexed_at = indexed_at \
             WHERE doc_id = ? AND indexed_at = ?::timestamptz",
            self.table(mapping)
        );
        let params = [Value::string(model), Value::string(id), Value::string(seen)];
        let count = self.tx.run(TransactionAttribute::UseOrBegin, None, || {
            self.tx.with_connection(|conn| conn.execute(&statement, &params))
        })?;
        Ok(count > 0)
    }

    fn ensure_index(&self, mapping: &DocumentMapping) -> Result<(), Error> {
        let signature = self.signature(mapping);
        if self.registered(&mapping.document_id)?.as_deref() == Some(signature.as_str()) {
            return Ok(());
        }
        // Feed workers index documents of one type at once; only one makes the table.
        let _ensuring = lock(&self.ensuring);
        lock(&self.known).remove(&mapping.document_id);
        lock(&self.absent).remove(&mapping.document_id);
        let current = self.registered(&mapping.document_id)?;
        if current.as_deref() == Some(signature.as_str()) {
            return Ok(());
        }
        let table = self.table(mapping);
        let registry = format!("{}.document_index", ident(&self.schema));
        let statements = self.create_statements(mapping);
        let advisory = format!("{}.{}", self.schema, mapping.physical_name);
        // True when this call made the table; false when another made it first.
        self.tx.with_reserve(|conn| {
            ddl_transaction(conn, &table, |conn| {
                // Another process may be making it: wait for it, then look again.
                conn.query("SELECT pg_advisory_xact_lock(hashtext(?))", &[Value::string(&advisory)])?;
                let rows = conn.query(
                    &format!("SELECT signature FROM {registry} WHERE document_id = ?"),
                    &[Value::string(&mapping.document_id)],
                )?;
                if rows.first().and_then(|row| row.get("signature")).map(to_plain_string).as_deref()
                    == Some(signature.as_str())
                {
                    return Ok(false);
                }
                if super::telemetry::is_telemetry_index(&mapping.document_id)||super::telemetry::is_telemetry_index(&mapping.physical_name){
                    let exists=conn.query("SELECT to_regclass(?)::text AS existing",&[Value::string(&table)])?.first().and_then(|row|row.get("existing")).is_some_and(|value|!value.is_null());
                    if !rows.is_empty()||exists{return Err(Error::new(format!("telemetry index {} mapping changed; existing history is preserved. Apply an explicit non-destructive schema migration and update the search registry before retrying",mapping.document_id)));}
                }
                if current.is_some() {
                    tracing::warn!(
                        document = %mapping.document_id,
                        "search store shape changed; the table is rebuilt and must be reindexed"
                    );
                }
                conn.execute(&format!("DROP TABLE IF EXISTS {table}"), &[])?;
                for statement in &statements {
                    conn.execute(statement, &[])?;
                }
                conn.execute(
                    &format!(
                        "INSERT INTO {registry} (document_id, index_name, physical_name, signature) VALUES (?, ?, ?, ?) \
                         ON CONFLICT (document_id) DO UPDATE SET index_name = EXCLUDED.index_name, \
                         physical_name = EXCLUDED.physical_name, signature = EXCLUDED.signature, created_at = now()"
                    ),
                    &[
                        Value::string(&mapping.document_id),
                        mapping.index_name.clone().map_or(Value::Null, Value::String),
                        Value::string(&mapping.physical_name),
                        Value::string(&signature),
                    ],
                )?;
                Ok(true)
            })
        })?;
        lock(&self.known).insert(mapping.document_id.clone(), signature);
        Ok(())
    }

    fn index_exists(&self, mapping: &DocumentMapping) -> Result<bool, Error> {
        Ok(self.registered(&mapping.document_id)?.is_some())
    }

    fn index_documents(&self, mapping: &DocumentMapping, documents: &[(String, Document)]) -> Result<usize, Error> {
        self.index_documents_at(mapping, documents, None)
    }

    /// `indexed_at` is the read time when there is one, and a row read later than this one is not replaced.
    fn index_documents_at(
        &self,
        mapping: &DocumentMapping,
        documents: &[(String, Document)],
        read_at: Option<Timestamp>,
    ) -> Result<usize, Error> {
        if documents.is_empty() {
            return Ok(0);
        }
        let table = self.table(mapping);
        let columns = insert_columns(mapping);
        let now = read_at.unwrap_or_else(Timestamp::now);
        // One statement holds a row once, so a document given twice is written as its last copy, as one statement per
        // document would leave it.
        let mut latest: IndexMap<&str, &Document> = IndexMap::with_capacity(documents.len());
        let mut generated = Vec::new();
        for (given, body) in documents {
            if given.is_empty() {
                // A log entry has no id: a time-ordered one is made here, as OpenSearch makes its own.
                generated.push((generated_doc_id(), body));
            } else {
                latest.insert(given.as_str(), body);
            }
        }
        let rows: Vec<(&str, &Document)> = latest
            .into_iter()
            .chain(generated.iter().map(|(id, body)| (id.as_str(), *body)))
            .collect();
        let per_row = 3 + columns.len();
        let chunk_rows = (MAX_INSERT_PARAMS / per_row).clamp(1, MAX_INSERT_ROWS);
        let mut statements = Vec::new();
        for chunk in rows.chunks(chunk_rows) {
            let mut params = Vec::with_capacity(chunk.len() * per_row);
            for (id, body) in chunk {
                push_insert_params(mapping, &columns, id, now, body, &mut params)?;
            }
            statements.push((insert_sql(&table, &columns, chunk.len(), read_at.is_some()), params));
        }
        self.tx.run(TransactionAttribute::UseOrBegin, None, || {
            self.tx.with_connection(|conn| {
                for (statement, params) in &statements {
                    conn.execute(statement, params)?;
                }
                Ok(())
            })
        })?;
        Ok(documents.len())
    }

    fn index_telemetry_batch(
        &self,
        mapping: &DocumentMapping,
        documents: &[(String, Document)],
    ) -> Result<Vec<super::TelemetryItem>, Error> {
        let columns = insert_columns(mapping);
        let now = Timestamp::now();
        let mut outcomes: Vec<_> = documents.iter().map(|_| super::TelemetryItem::Accepted).collect();
        let mut valid = Vec::new();
        let mut positions = Vec::new();
        for (position, document) in documents.iter().enumerate() {
            match push_insert_params(mapping, &columns, &document.0, now, &document.1, &mut Vec::new()) {
                Ok(()) => {
                    valid.push(document.clone());
                    positions.push(position);
                }
                Err(error) => outcomes[position] = super::TelemetryItem::Rejected(error.to_string()),
            }
        }
        if valid.is_empty() {
            return Ok(outcomes);
        }
        match self.index_documents(mapping, &valid) {
            Ok(_) => {}
            Err(error) if super::telemetry_record_error(&error) => {
                for (position, document) in positions.into_iter().zip(&valid) {
                    outcomes[position] = match self.index_documents(mapping, std::slice::from_ref(document)) {
                        Ok(_) => super::TelemetryItem::Accepted,
                        Err(error) if super::telemetry_record_error(&error) => {
                            super::TelemetryItem::Rejected(error.to_string())
                        }
                        Err(error) => super::TelemetryItem::Retryable(error.to_string()),
                    };
                }
            }
            Err(error) => {
                for position in positions {
                    outcomes[position] = super::TelemetryItem::Retryable(error.to_string());
                }
            }
        }
        Ok(outcomes)
    }

    fn delete_document(&self, mapping: &DocumentMapping, id: &str) -> Result<bool, Error> {
        let table = self.table(mapping);
        let count = self.tx.run(TransactionAttribute::UseOrBegin, None, || {
            self.tx.with_connection(|conn| {
                conn.execute(&format!("DELETE FROM {table} WHERE doc_id = ?"), &[Value::string(id)])
            })
        })?;
        Ok(count > 0)
    }

    fn delete_matching(&self, mapping: &DocumentMapping, filter: &Query) -> Result<u64, Error> {
        let compiler = Compiler::with_text(mapping, &self.text);
        let mut sql = Sql::new(format!("DELETE FROM {} WHERE ", self.table(mapping)));
        sql.append(compiler.where_sql(filter)?);
        self.tx.run(TransactionAttribute::UseOrBegin, None, || {
            self.tx.with_connection(|conn| conn.execute(&sql.text, &sql.params))
        })
    }

    fn get_document(&self, mapping: &DocumentMapping, id: &str) -> Result<Option<Document>, Error> {
        let mut sql = Sql::new(format!(
            "SELECT body::text AS body FROM {} WHERE doc_id = ",
            self.table(mapping)
        ));
        sql.param(Value::string(id));
        let rows = self.query_on(self.read_route(None), &sql)?;
        rows.first().map(|row| parse_body(row.get("body"))).transpose()
    }

    fn search(&self, mappings: &[Arc<DocumentMapping>], request: &SearchRequest) -> Result<SearchPage, Error> {
        let plan = search_sql(&self.schema, &self.text, mappings, request)?;
        let route = self.read_route(request.fresh_by);
        let total = self
            .query_on(route, &plan.count)?
            .first()
            .and_then(|row| row.get("total"))
            .map(|value| to_plain_string(value).parse::<u64>().unwrap_or(0))
            .unwrap_or(0);
        let mut hits = Vec::new();
        if request.limit > 0 && total > 0 {
            for row in self.query_on(route, &plan.select)? {
                let mut body = parse_body(row.get("body"))?;
                if request.sort.last().is_some_and(|s| s.field == "__cursor") {
                    for (index, sort) in request.sort.iter().enumerate().filter(|(_, s)| s.field != "__cursor") {
                        if !body.contains_key(&sort.field) {
                            if let Some(value) = row.get(&format!("s{index}")) {
                                body.insert(sort.field.clone(), value.clone());
                            }
                        }
                    }
                }
                let highlights = if request.highlight.is_empty() {
                    IndexMap::new()
                } else {
                    highlights(&request.query, &request.highlight, &body)
                };
                hits.push(SearchHit {
                    document_id: row.get("document_id").map(to_plain_string).unwrap_or_default(),
                    id: row.get("doc_id").map(to_plain_string).unwrap_or_default(),
                    index: row.get("index_name").map(to_plain_string).unwrap_or_default(),
                    score: row
                        .get("score")
                        .map(|value| to_plain_string(value).parse::<f64>().unwrap_or(0.0))
                        .unwrap_or(0.0),
                    body,
                    highlights,
                });
            }
        }
        Ok(SearchPage {
            hits,
            total,
            total_relation: super::TotalRelation::Eq,
        })
    }

    fn aggregate(
        &self,
        mapping: &DocumentMapping,
        filter: &Query,
        aggs: &IndexMap<String, Agg>,
    ) -> Result<IndexMap<String, Value>, Error> {
        self.aggregate_fresh(mapping, filter, aggs, None)
    }

    fn aggregate_fresh(
        &self,
        mapping: &DocumentMapping,
        filter: &Query,
        aggs: &IndexMap<String, Agg>,
        fresh_by: Option<Instant>,
    ) -> Result<IndexMap<String, Value>, Error> {
        let table = self.table(mapping);
        let route = self.read_route(fresh_by);
        let mut run = |sql: &Sql| self.query_on(route, sql);
        let mut result = agg::aggregate(&table, mapping, &self.text, filter, aggs, &mut run)?;
        super::agg::fill_other_buckets(self, mapping, filter, aggs, &mut result)?;
        Ok(result)
    }

    fn aggregate_many(
        &self,
        mappings: &[Arc<DocumentMapping>],
        filter: &Query,
        aggs: &IndexMap<String, Agg>,
        fresh_by: Option<Instant>,
    ) -> Result<IndexMap<String, Value>, Error> {
        let mapping = super::mapping::combined_mapping(mappings)?;
        let mut population = Sql::new("");
        for (index, source) in mappings.iter().enumerate() {
            if index > 0 {
                population.push(" UNION ALL ");
            }
            population.push(&format!(
                "SELECT doc_id, indexed_at, body, {}::text AS __document, all_text, ",
                literal(&source.document_id)
            ));
            population.push(if !source.display.weights.is_empty() {
                RANKED_TEXT
            } else {
                "all_text"
            });
            population.push(&format!(" AS {RANKED_TEXT}, "));
            population.push(if Compiler::with_text(source, &self.text).title_field().is_some() {
                NORM_TITLE
            } else {
                "NULL::text"
            });
            population.push(&format!(" AS {NORM_TITLE}"));
            for (name, field) in &mapping.root.fields {
                if let Some(kind) = column_type(field.kind) {
                    let column = column(name);
                    population.push(", ");
                    if source
                        .root
                        .fields
                        .get(name)
                        .is_some_and(|field| column_type(field.kind).is_some())
                    {
                        population.push(&format!("{column}::{kind}"));
                    } else {
                        population.push(&format!("NULL::{kind}"));
                    }
                    population.push(&format!(" AS {column}"));
                }
            }
            population.push(&format!(" FROM {} WHERE ", self.table(source)));
            population.append(Compiler::with_text(source, &self.text).where_sql(filter)?);
        }
        let route = self.read_route(fresh_by);
        let mut run = |sql: &Sql| {
            let mut combined = Sql::new("WITH moqui_population AS MATERIALIZED (");
            combined.append(population.clone());
            combined.push(") ");
            combined.append(sql.clone());
            self.query_on(route, &combined)
        };
        let mut result = agg::aggregate_population(
            "moqui_population",
            &mapping,
            &self.text,
            &Query::MatchAll,
            aggs,
            Some(mappings),
            &mut run,
        )?;
        let mut again =
            |filter: &Query, aggs: &IndexMap<String, Agg>| self.aggregate_many(mappings, filter, aggs, fresh_by);
        super::agg::fill_other_buckets_with(&mut again, filter, aggs, &mut result)?;
        Ok(result)
    }

    fn replica_status(&self) -> Option<ReplicaStatus> {
        let read = self.read.as_ref()?;
        let lag_seconds = read
            .with_connection(|conn| {
                conn.query(
                    "SELECT EXTRACT(EPOCH FROM (now() - pg_last_xact_replay_timestamp()))::float8 AS lag",
                    &[],
                )
            })
            .ok()
            .and_then(|rows| rows.first().and_then(|row| row.get("lag")).cloned())
            .and_then(|value| to_plain_string(&value).parse::<f64>().ok());
        Some(ReplicaStatus {
            lag_seconds,
            primary_fallbacks: self.fallbacks.load(Ordering::Relaxed),
        })
    }

    fn drop_index(&self, mapping: &DocumentMapping) -> Result<(), Error> {
        let table = self.table(mapping);
        let registry = format!("{}.document_index", ident(&self.schema));
        self.tx.with_reserve(|conn| {
            ddl_transaction(conn, &table, |conn| {
                conn.execute(&format!("DROP TABLE IF EXISTS {table}"), &[])?;
                conn.execute(
                    &format!("DELETE FROM {registry} WHERE document_id = ?"),
                    &[Value::string(&mapping.document_id)],
                )?;
                Ok(())
            })
        })?;
        lock(&self.known).remove(&mapping.document_id);
        lock(&self.absent).remove(&mapping.document_id);
        Ok(())
    }
}

/// How long table DDL on a reserve connection waits for a lock. The reserve connection is not the
/// caller's, so a lock the caller's own open transaction holds on the table would otherwise be
/// waited for without end.
const DDL_LOCK_TIMEOUT: &str = "5s";

/// Run `work` in a transaction on a reserve connection, with [`DDL_LOCK_TIMEOUT`]. A lock wait that
/// runs out is an error that names the table.
fn ddl_transaction<T>(
    conn: &mut dyn DbConnection,
    table: &str,
    work: impl FnOnce(&mut dyn DbConnection) -> Result<T, Error>,
) -> Result<T, Error> {
    conn.begin()?;
    let result = conn
        .execute(&format!("SET LOCAL lock_timeout = '{DDL_LOCK_TIMEOUT}'"), &[])
        .and_then(|_| work(conn));
    match result {
        Ok(value) => conn.commit().map(|()| value),
        Err(err) => {
            let _ = conn.rollback();
            if err.message().contains("lock timeout") {
                Err(Error::new(format!(
                    "search table {table} stayed locked for {DDL_LOCK_TIMEOUT}; a transaction that has written to it, \
                     perhaps the caller's own, must end before the table is made or dropped"
                ))
                .with_source(err))
            } else {
                Err(err)
            }
        }
    }
}

/// The page statement and the count statement for a search.
pub struct SearchPlan {
    pub select: Sql,
    pub count: Sql,
}

/// Build the search statements. One document is one `SELECT`; several are a `UNION ALL`.
///
/// # Errors
///
/// Returns an error for a sort on a text field that is not sortable, or a value that does not fit
/// its field.
pub fn search_sql(
    schema: &str,
    text: &TextSetup,
    mappings: &[Arc<DocumentMapping>],
    request: &SearchRequest,
) -> Result<SearchPlan, Error> {
    // The kind of each sort key, from the first document that has the field.
    let mut sort_kinds: Vec<Option<FieldKind>> = Vec::new();
    for sort in &request.sort {
        let mut kind = None;
        for mapping in mappings {
            match sort.field.as_str() {
                "_score" | "_id" | "__cursor" => break,
                _ => {}
            }
            let compiler = Compiler::new(mapping);
            if let Located::Field { field, .. } = compiler.locate_root(&sort.field) {
                if !field.can_sort() {
                    return Err(Error::new(format!(
                        "{} is a text field that is not sortable",
                        sort.field
                    )));
                }
                kind = match kind {
                    None => Some(field.kind),
                    Some(existing) if existing == field.kind => Some(existing),
                    Some(_) => Some(FieldKind::Keyword),
                };
            }
        }
        sort_kinds.push(kind);
    }

    let mut parts = Vec::new();
    let mut counts = Vec::new();
    for mapping in mappings {
        let compiler = Compiler::with_text(mapping, text);
        let table = format!("{}.{}", ident(schema), ident(&mapping.physical_name));
        // The trusted filter narrows the rows; only the query scores.
        let where_sql = compiler.where_sql(&request.matching())?;
        let mut part = Sql::new(format!(
            "SELECT {} AS document_id, {} AS index_name, doc_id, body::text AS body, ",
            literal(&mapping.document_id),
            literal(&mapping.physical_name)
        ));
        part.push("(");
        if crate::search::query::has_score_extensions(&request.query) {
            part.append(compiler.score_sql(&request.query, request.rank)?);
        } else {
            match compiler.rank_query(&request.query) {
                // Cover density over the weighted vector when the document has weights, else all text.
                Some(rank) if request.rank == Rank::CoverDensity => {
                    let vector = if mapping.display.weights.is_empty() {
                        "all_text"
                    } else {
                        RANKED_TEXT
                    };
                    part.push(&format!("ts_rank_cd({vector}, "));
                    part.append(rank);
                    part.push(")");
                }
                Some(rank) => {
                    part.push("ts_rank(all_text, ");
                    part.append(rank);
                    part.push(")");
                }
                None => {
                    part.push("0");
                }
            }
            for extra in compiler.extra_score(&request.query) {
                part.push(" + ");
                part.append(extra);
            }
        }
        part.push(")::float8 AS score");
        for (index, (sort, kind)) in request.sort.iter().zip(&sort_kinds).enumerate() {
            let expr = if sort.field == "__cursor" {
                format!("{} || ':' || doc_id", literal(&mapping.document_id))
            } else {
                sort_expr(&compiler, sort.field.as_str(), sort.descending, *kind)
            };
            part.push(&format!(", {expr} AS s{index}"));
        }
        part.push(&format!(" FROM {table} WHERE "));
        part.append(where_sql.clone());
        parts.push(part);

        let mut count = Sql::new(format!("(SELECT count(*) FROM {table} WHERE "));
        count.append(where_sql);
        count.push(")");
        counts.push(count);
    }

    let mut order = Vec::new();
    for (index, sort) in request.sort.iter().enumerate() {
        let direction = if sort.descending { "DESC" } else { "ASC" };
        order.push(format!("s{index} {direction} NULLS LAST"));
    }
    if request.sort.is_empty() {
        order.push("score DESC".to_string());
    }
    order.push("document_id ASC".to_string());
    order.push("doc_id ASC".to_string());

    let mut select = if parts.len() == 1 {
        parts.pop().unwrap_or_default()
    } else {
        let mut union = Sql::new("SELECT * FROM (");
        for (index, part) in parts.into_iter().enumerate() {
            if index > 0 {
                union.push(" UNION ALL ");
            }
            union.append(part);
        }
        union.push(") AS u");
        union
    };
    if let Some(after) = &request.search_after {
        let mut cursor = Sql::new("SELECT * FROM (");
        cursor.append(select);
        cursor.push(") AS cursor_page WHERE (");
        for index in 0..request.sort.len() {
            if index > 0 {
                cursor.push(" OR ");
            }
            cursor.push("(");
            for prior in 0..index {
                if prior > 0 {
                    cursor.push(" AND ");
                }
                cursor.push(&format!("s{prior} = "));
                cursor.param(cursor_value(&after[prior], sort_kinds[prior]));
            }
            if index > 0 {
                cursor.push(" AND ");
            }
            cursor.push(&format!(
                "s{index} {} ",
                if request.sort[index].descending { "<" } else { ">" }
            ));
            cursor.param(cursor_value(&after[index], sort_kinds[index]));
            cursor.push(")");
        }
        cursor.push(")");
        select = cursor;
    }
    select.push(&format!(
        " ORDER BY {} LIMIT {} OFFSET {}",
        order.join(", "),
        request.limit,
        request.offset
    ));

    let mut count = Sql::new("SELECT ");
    for (index, part) in counts.into_iter().enumerate() {
        if index > 0 {
            count.push(" + ");
        }
        count.append(part);
    }
    count.push(" AS total");
    Ok(SearchPlan { select, count })
}

fn cursor_value(value: &Value, kind: Option<FieldKind>) -> Value {
    if kind == Some(FieldKind::Date) {
        compile::parse_date_bound(value)
            .map(Value::Timestamp)
            .unwrap_or_else(|| value.clone())
    } else if kind == Some(FieldKind::Long) {
        to_plain_string(value)
            .parse::<i64>()
            .map(Value::I64)
            .unwrap_or_else(|_| value.clone())
    } else {
        value.clone()
    }
}

fn sort_expr(compiler: &Compiler<'_>, field: &str, descending: bool, kind: Option<FieldKind>) -> String {
    let null_of = |kind: Option<FieldKind>| match kind.and_then(column_type) {
        Some(sql_type) => format!("NULL::{sql_type}"),
        None => "NULL::text".to_string(),
    };
    match field {
        "_score" => return "score".to_string(),
        "_id" => return "doc_id".to_string(),
        _ => {}
    }
    match compiler.locate_root(field) {
        Located::Missing => null_of(kind),
        Located::Field {
            arrays,
            expr,
            field: found,
        } => {
            let expr = if kind.is_some_and(|kind| kind != found.kind) {
                format!("({expr})::text")
            } else {
                expr
            };
            if arrays.is_empty() {
                expr
            } else {
                // A child array sorts by its smallest value ascending and its largest descending.
                let function = if descending { "max" } else { "min" };
                let from = arrays
                    .iter()
                    .map(|(var, source)| format!("{source} AS {var}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("(SELECT {function}({expr}) FROM {from})")
            }
        }
    }
}

fn parse_body(value: Option<&Value>) -> Result<Document, Error> {
    let text = value.map(to_plain_string).unwrap_or_default();
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|err| Error::new(format!("stored document is not JSON: {err}")))?;
    match moqui_lib::json::from_json(&json) {
        Value::Map(map) => Ok(map),
        _ => Err(Error::new("stored document is not an object")),
    }
}

/// JSON for a document body. Timestamps are ISO-8601 UTC, dates are `yyyy-MM-dd`, decimals
/// stay exact.
pub fn body_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Timestamp(at) => serde_json::Value::String(at.to_string()),
        Value::Date(date) => serde_json::Value::String(date.to_string()),
        Value::Time(time) => serde_json::Value::String(time.to_string()),
        Value::List(items) => serde_json::Value::Array(items.iter().map(body_json).collect()),
        Value::Map(entries) => body_json_map(entries),
        other => moqui_lib::json::to_json(other),
    }
}

/// [`body_json`] of a document's fields, without wrapping them in a [`Value::Map`].
pub fn body_json_map(entries: &Document) -> serde_json::Value {
    serde_json::Value::Object(
        entries
            .iter()
            .filter(|(_, item)| !item.is_null())
            .map(|(key, item)| (key.clone(), body_json(item)))
            .collect(),
    )
}

#[cfg(test)]
mod tests;
