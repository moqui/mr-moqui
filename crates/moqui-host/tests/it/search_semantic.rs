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

//! Semantic storage and retrieval on a Postgres that has `pgvector`, which embedded Postgres does
//! not bundle. Ignored unless `PGVECTOR_URL` names one (`postgresql://host:port/database`, with
//! `PGVECTOR_USER` and `PGVECTOR_PASSWORD`; `?sslmode=disable` for a local server). The embedder is
//! the development hash embedder, so this checks storage, the stale guard, the worker queries, the
//! nearest-neighbor search, and its filter, not meaning.

#![cfg(feature = "postgres")]

use std::sync::Arc;
use std::time::Duration;

use moqui_core::entity::document::Document;
use moqui_core::search::mapping::{DisplayFields, DocumentMapping, FieldKind, FieldMapping, ObjectMapping};
use moqui_core::search::postgres::PostgresBackend;
use moqui_core::search::semantic::{Embedder, HashEmbedder, SemanticConf};
use moqui_core::search::{Capability, Query, SearchBackend};
use moqui_core::transaction::{PoolConfig, TransactionManager};
use moqui_lib::object::Value;

#[test]
#[ignore = "needs pgvector: PGVECTOR_URL=postgresql://127.0.0.1:5432/db?sslmode=disable cargo test -p moqui-host --test it -- search_semantic --ignored"]
fn embeddings_are_stored_guarded_and_searched() {
    let url = std::env::var("PGVECTOR_URL").expect("set PGVECTOR_URL");
    let parsed = moqui_core::search::conf::parse_postgres_url(&url).unwrap();
    let user = std::env::var("PGVECTOR_USER").unwrap_or_else(|_| "postgres".into());
    let password = std::env::var("PGVECTOR_PASSWORD").ok();
    let driver = moqui_host::db::entity_db::postgres_server_driver(
        &user,
        password.as_deref(),
        &parsed.database,
        &parsed.host,
        parsed.port,
        parsed
            .tls
            .then(|| moqui_host::db::pg_tls::TlsOptions::new(parsed.tls_verify, &parsed.tls_root_cert))
            .as_ref(),
    )
    .unwrap();
    let pool = PoolConfig {
        min: 0,
        max: 4,
        reserve: 1,
        acquire_timeout: Duration::from_secs(30),
        ..Default::default()
    };
    let tx = TransactionManager::new(Arc::new(driver), pool).unwrap();
    let conf = SemanticConf {
        enabled: true,
        dimensions: 64,
        representation: "vector".into(),
        ..SemanticConf::default()
    };
    let schema = format!("search_semantic_{}", std::process::id());
    let backend = PostgresBackend::open(tx, Some(&schema))
        .unwrap()
        .with_semantic(&conf)
        .unwrap();
    assert!(
        backend.capabilities().contains(&Capability::Semantic),
        "pgvector is installed"
    );
    let embedder = HashEmbedder::new(64);

    let mut root = ObjectMapping::default();
    for (name, kind) in [("itemId", FieldKind::Keyword), ("title", FieldKind::Text)] {
        root.fields.insert(name.into(), FieldMapping { kind, sortable: false });
    }
    let mut display = DisplayFields::default();
    display.roles.insert("title".into(), "title".into());
    let mapping = DocumentMapping {
        document_id: "TestSemanticItem".into(),
        index_name: None,
        physical_name: "test_semantic_item".into(),
        root,
        manual_mapping_service: None,
        display,
    };
    backend.ensure_index(&mapping).unwrap();
    let item = |id: &str, title: &str| -> (String, Document) {
        let mut body = Document::new();
        body.insert("itemId".into(), Value::string(id));
        body.insert("title".into(), Value::string(title));
        (id.to_string(), body)
    };
    backend
        .index_documents(
            &mapping,
            &[
                item("A", "water pump"),
                item("B", "garden hose"),
                item("C", "water heater"),
            ],
        )
        .unwrap();

    // The worker: every document is pending, then none.
    let embed_all = || {
        let pending = backend.pending_embeddings(&mapping, embedder.model(), 100).unwrap();
        let texts: Vec<String> = pending
            .iter()
            .map(|item| moqui_core::search::semantic::embedding_text(&mapping, &item.body, 2000))
            .collect();
        let vectors = embedder.embed(&texts).unwrap();
        for (item, vector) in pending.iter().zip(vectors) {
            assert!(backend
                .store_embedding(&mapping, &item.id, &item.seen, embedder.model(), &vector)
                .unwrap());
        }
        pending.len()
    };
    assert_eq!(embed_all(), 3);
    assert!(backend
        .pending_embeddings(&mapping, embedder.model(), 100)
        .unwrap()
        .is_empty());

    // Nearest neighbors, and the filter.
    let mappings = vec![Arc::new(mapping.clone())];
    let vector = embedder.embed(&["water pump".to_string()]).unwrap().remove(0);
    let hits = backend
        .semantic_search(&mappings, &vector, embedder.model(), None, 2)
        .unwrap();
    assert_eq!(hits[0].id, "A");
    let filter = Query::eq("itemId", Value::string("C"));
    let hits = backend
        .semantic_search(&mappings, &vector, embedder.model(), Some(&filter), 10)
        .unwrap();
    assert_eq!(hits.iter().map(|hit| hit.id.as_str()).collect::<Vec<_>>(), vec!["C"]);

    // The stale guard: a body indexed again after the worker read it keeps the worker's vector out.
    let stale = backend
        .pending_embeddings(&mapping, "another-model", 1)
        .unwrap()
        .remove(0);
    std::thread::sleep(Duration::from_millis(5));
    backend
        .index_documents(&mapping, &[item(&stale.id, "changed title")])
        .unwrap();
    assert!(!backend
        .store_embedding(&mapping, &stale.id, &stale.seen, "another-model", &vector)
        .unwrap());
    // The re-indexed document is pending again for the current model.
    assert_eq!(embed_all(), 1);

    backend.drop_index(&mapping).unwrap();
}
