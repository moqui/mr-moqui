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

use super::*;

#[test]
fn document_type_buckets_work_beside_metrics_and_inside_other_buckets() {
    let source = serde_json::json!({"total":{"sum":{"field":"amount"}},"region":{"terms":{"field":"region"},"aggs":{"types":{"terms":{"field":"_document"},"aggs":{"amount":{"sum":{"field":"amount"}}}}}}});
    let aggs = Agg::map_from_value(&moqui_lib::json::from_json(&source)).unwrap();
    let compiled = compile_document_terms(&aggs);
    assert!(matches!(
        compiled["region"].sub_aggs().unwrap()["types"],
        Agg::Filter { .. }
    ));
    let response = serde_json::json!({"total":10,"region":{"buckets":[{"key":"A","doc_count":2,"types":{"doc_count":2,"amount":10}}]}});
    let Value::Map(mut response) = moqui_lib::json::from_json(&response) else {
        panic!("map")
    };
    restore_document_terms(&aggs, &mut response, "ReportParty");
    let response = moqui_lib::json::to_json(&Value::Map(response));
    let bucket = &response["region"]["buckets"][0]["types"]["buckets"][0];
    assert_eq!(bucket["key"], "ReportParty");
    assert_eq!(bucket["doc_count"], 2);
    assert_eq!(bucket["amount"], 10);
    assert_eq!(response["total"], 10);
}

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
    fn capabilities(&self) -> Vec<Capability> {
        vec![Capability::Semantic]
    }
    fn ensure_index(&self, _mapping: &DocumentMapping) -> Result<(), Error> {
        Ok(())
    }
    fn index_exists(&self, _mapping: &DocumentMapping) -> Result<bool, Error> {
        Ok(true)
    }
    fn index_documents(&self, _mapping: &DocumentMapping, _documents: &[(String, Document)]) -> Result<usize, Error> {
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
    let result = facade.embed_pending(&Target::Documents(vec![telemetry::LOGS.to_string()]), 10, 100);
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
    let first = facade.mapping(telemetry::LOGS).unwrap();
    assert!(Arc::ptr_eq(&first, &facade.mapping(telemetry::LOGS).unwrap()));
    let mut document = entity.make_value("moqui.entity.document.DataDocument").unwrap();
    document.set("dataDocumentId", Value::string("REFRESHED"));
    document.set("primaryEntityName", Value::string("moqui.basic.Enumeration"));
    document.create(&entity).unwrap();
    assert!(!Arc::ptr_eq(&first, &facade.mapping(telemetry::LOGS).unwrap()));
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

#[test]
fn telemetry_retries_only_unacknowledged_items_with_stable_ids() {
    let documents = ["accepted", "transient", "poison"].map(|id| (id.into(), Document::new()));
    let mut calls = Vec::new();
    let mut waits = Vec::new();
    let persisted = retry_telemetry_batch(
        &documents,
        |batch| {
            calls.push(batch.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>());
            Ok(batch
                .iter()
                .map(|(id, _)| match (id.as_str(), calls.len()) {
                    ("poison", _) => TelemetryItem::Rejected("bad record".into()),
                    ("transient", 1) => TelemetryItem::Retryable("offline".into()),
                    _ => TelemetryItem::Accepted,
                })
                .collect())
        },
        |delay| waits.push(delay.as_millis()),
        "otel_logs",
    )
    .unwrap();
    assert_eq!(persisted, 2);
    assert_eq!(calls, vec![vec!["accepted", "transient", "poison"], vec!["transient"]]);
    assert_eq!(waits, [100]);
}

#[test]
fn ambiguous_telemetry_writes_are_idempotent_and_retries_are_capped() {
    let documents = [("stable-id".into(), Document::new())];
    let mut stored = std::collections::HashSet::new();
    let mut calls = 0;
    let persisted = retry_telemetry_batch(
        &documents,
        |batch| {
            calls += 1;
            stored.extend(batch.iter().map(|(id, _)| id.clone()));
            if calls < 3 {
                Err(Error::new("response lost after commit"))
            } else {
                Ok(vec![TelemetryItem::Accepted])
            }
        },
        |_| {},
        "otel_logs",
    )
    .unwrap();
    assert_eq!((persisted, stored.len(), calls), (1, 1, 3));
    calls = 0;
    let mut waits = Vec::new();
    assert_eq!(
        retry_telemetry_batch(
            &documents,
            |_| {
                calls += 1;
                Err(Error::new("offline"))
            },
            |delay| waits.push(delay.as_millis()),
            "otel_logs"
        )
        .unwrap(),
        0
    );
    assert_eq!(calls, 3);
    assert_eq!(waits, [100, 500]);
}
