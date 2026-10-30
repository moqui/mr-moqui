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

#[test]
fn generated_ids_are_unique_and_follow_arrival_order() {
    let first = super::generated_doc_id();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let second = super::generated_doc_id();
    assert_eq!(first.len(), 25);
    assert_ne!(first, second);
    assert!(first[..13] < second[..13]);
}

use super::*;
use crate::search::mapping::{FieldMapping, ObjectMapping};
use crate::search::{SortField, Target};

fn mapping(id: &str, physical: &str) -> Arc<DocumentMapping> {
    let mut root = ObjectMapping::default();
    root.fields.insert(
        "combinedName".into(),
        FieldMapping {
            kind: FieldKind::Text,
            sortable: true,
        },
    );
    let mut accounts = ObjectMapping::default();
    accounts.fields.insert(
        "username".into(),
        FieldMapping {
            kind: FieldKind::Text,
            sortable: true,
        },
    );
    root.children.insert("userAccounts".into(), accounts);
    Arc::new(DocumentMapping {
        document_id: id.into(),
        index_name: Some("mantle".into()),
        physical_name: physical.into(),
        root,
        manual_mapping_service: None,
        display: Default::default(),
    })
}

#[test]
fn union_search_sorts_child_by_max_descending() {
    let mappings = vec![
        mapping("MantleParty", "mantle_party"),
        mapping("MantleProduct", "mantle_product"),
    ];
    let mut request = SearchRequest::new(crate::search::parse::parse_query_string("*:jo*").unwrap());
    request.sort = vec![SortField::parse("-userAccounts.username.keyword").unwrap()];
    request.limit = 20;
    let plan = search_sql("search", &TextSetup::default(), &mappings, &request).unwrap();
    assert!(
        plan.select
            .text
            .starts_with("SELECT * FROM (SELECT 'MantleParty' AS document_id"),
        "{}",
        plan.select.text
    );
    assert!(plan
        .select
        .text
        .contains("(SELECT max((e0 ->> 'username')) FROM jsonb_array_elements(body -> 'userAccounts') AS e0) AS s0"));
    assert!(plan
        .select
        .text
        .ends_with("ORDER BY s0 DESC NULLS LAST, document_id ASC, doc_id ASC LIMIT 20 OFFSET 0"));
    assert_eq!(plan.select.params.len(), 4);
    assert!(plan
        .count
        .text
        .starts_with("SELECT (SELECT count(*) FROM \"search\".\"mantle_party\" WHERE"));
    let _ = Target::Index("mantle".into());
}

#[test]
fn documents_insert_as_rows_of_one_statement_with_a_cast_per_typed_column() {
    let mut mapping = (*mapping("MantleParty", "mantle_party")).clone();
    for (name, kind) in [
        ("partyCount", FieldKind::Long),
        ("photo", FieldKind::Binary),
        ("active", FieldKind::Boolean),
    ] {
        mapping
            .root
            .fields
            .insert(name.into(), FieldMapping { kind, sortable: false });
    }
    let columns = insert_columns(&mapping);
    let names: Vec<(&str, &str)> = columns.iter().map(|item| (item.name, item.cast)).collect();
    assert_eq!(
        names,
        [
            ("combinedName", ""),
            ("partyCount", "::bigint"),
            ("active", "::boolean")
        ]
    );
    let row = "(?, ?::timestamptz, ?::jsonb, ?, ?::bigint, ?::boolean)";
    let update = "ON CONFLICT (doc_id) DO UPDATE SET indexed_at = EXCLUDED.indexed_at, body = EXCLUDED.body, \
             \"f_combinedName\" = EXCLUDED.\"f_combinedName\", \"f_partyCount\" = EXCLUDED.\"f_partyCount\", \
             \"f_active\" = EXCLUDED.\"f_active\"";
    assert_eq!(
        insert_sql("t", "doc_id", &columns, 2, false),
        format!(
            "INSERT INTO t (doc_id, indexed_at, body, \"f_combinedName\", \"f_partyCount\", \"f_active\") \
                 VALUES {row}, {row} {update}"
        )
    );
    assert!(insert_sql("t", "doc_id", &columns, 1, true).ends_with(" WHERE t.indexed_at <= EXCLUDED.indexed_at"));

    let at: Timestamp = "2026-01-05T10:00:00Z".parse().unwrap();
    let mut body = Document::new();
    body.insert("combinedName".into(), Value::string("Ada"));
    body.insert("partyCount".into(), Value::string("3.0"));
    body.insert("active".into(), Value::Null);
    let mut params = Vec::new();
    push_insert_params(&mapping, &columns, "P1", at, &body, &mut params).unwrap();
    assert_eq!(
        params,
        [
            Value::string("P1"),
            Value::Timestamp(at),
            Value::string(r#"{"combinedName":"Ada","partyCount":"3.0"}"#),
            Value::string("Ada"),
            Value::string("3"),
            Value::Null,
        ]
    );
}

/// A date a script indexes is epoch-millis text (`index#DataDocuments` in JavaScript); in a child element it is stored
/// as ISO-8601 text, which casts to timestamptz, and the root column takes the same time. A `yyyy-MM-dd` date and
/// text that is not a time stay as they were.
#[test]
fn dates_given_as_text_are_stored_as_times_at_the_root_and_in_child_elements() {
    let date = FieldMapping {
        kind: FieldKind::Date,
        sortable: false,
    };
    let mut mapping = (*mapping("MantleParty", "mantle_party")).clone();
    mapping.root.fields.insert("createdDate".into(), date);
    let mut classes = ObjectMapping::default();
    classes.fields.insert("classFromDate".into(), date);
    classes.fields.insert("classThruDate".into(), date);
    mapping.root.children.insert("classifications".into(), classes);
    let columns = insert_columns(&mapping);
    let element = |from: &str, thru: &str| {
        let mut element = Document::new();
        element.insert("classFromDate".into(), Value::string(from));
        element.insert("classThruDate".into(), Value::string(thru));
        Value::Map(element)
    };
    let mut body = Document::new();
    body.insert("createdDate".into(), Value::string("1767607200000"));
    body.insert(
        "classifications".into(),
        Value::List(vec![
            element("1767607200000", "2026-02-01"),
            element("2026-01-05 10:00:00.000", "soon"),
        ]),
    );
    let at: Timestamp = "2026-01-05T10:00:00Z".parse().unwrap();
    let mut params = Vec::new();
    push_insert_params(&mapping, &columns, "P1", at, &body, &mut params).unwrap();
    assert_eq!(
        params[2],
        Value::string(
            r#"{"classifications":[{"classFromDate":"2026-01-05T10:00:00Z","classThruDate":"2026-02-01"},{"classFromDate":"2026-01-05T10:00:00Z","classThruDate":"soon"}],"createdDate":"2026-01-05T10:00:00Z"}"#
        )
    );
    let created = columns.iter().position(|column| column.name == "createdDate").unwrap();
    assert_eq!(params[3 + created], Value::Timestamp(at));
}

#[test]
fn body_json_keeps_dates_and_drops_nulls() {
    let mut body = Document::new();
    body.insert(
        "placedDate".into(),
        Value::Timestamp("2026-01-05T10:00:00Z".parse().unwrap()),
    );
    body.insert("missing".into(), Value::Null);
    let json = body_json(&Value::Map(body));
    assert_eq!(json.to_string(), r#"{"placedDate":"2026-01-05T10:00:00Z"}"#);
}

#[test]
fn semantic_sql_ranks_by_cosine_distance_per_type_with_the_filter() {
    let mut mapping = super::super::telemetry::telemetry_mapping("otel_logs").unwrap();
    mapping.document_id = "TestDoc".into();
    mapping.physical_name = "test_doc".into();
    let store = VectorStore {
        representation: "halfvec".into(),
        dimensions: 3,
    };
    let filter = Query::eq("severity_text", Value::string("WARN"));
    let sql = semantic_sql(
        "search",
        &TextSetup::default(),
        &store,
        &[Arc::new(mapping)],
        &SemanticQuery {
            vector: &[0.5, 0.25, 0.0],
            model: "moqui-hash",
            filter: Some(&filter),
            k: 10,
        },
    )
    .unwrap();
    assert_eq!(
        sql.text,
        "SELECT * FROM ((SELECT 'TestDoc' AS document_id, 'test_doc' AS index_name, doc_id, body::text AS body, \
             (1 - (embedding <=> ?::halfvec(3)))::float8 AS score FROM \"search\".\"test_doc\" WHERE embedding IS NOT NULL \
             AND embedding_model = ? AND COALESCE(COALESCE(\"f_severity_text\" = ?, (body -> 'severity_text') @> jsonb_build_array(?::text)), FALSE) \
             ORDER BY embedding <=> ?::halfvec(3) LIMIT 10)) AS s \
             ORDER BY score DESC, document_id ASC, doc_id ASC LIMIT 10"
    );
    assert_eq!(sql.params[0], Value::string("[0.5,0.25,0]"));
    assert_eq!(sql.params[1], Value::string("moqui-hash"));
}

/// A database with no rows that counts the registry lookups it is sent.
struct Registry(Arc<std::sync::atomic::AtomicUsize>);

struct RegistryConn(Arc<std::sync::atomic::AtomicUsize>);

struct NoCancel;

impl crate::driver::DbCancel for NoCancel {
    fn cancel(&self) -> Result<(), Error> {
        Ok(())
    }
}

impl crate::driver::DbDriver for Registry {
    fn connect(&self) -> Result<Box<dyn crate::driver::DbConnection>, Error> {
        Ok(Box::new(RegistryConn(Arc::clone(&self.0))))
    }
}

impl crate::driver::DbConnection for RegistryConn {
    fn execute(&mut self, _sql: &str, _params: &[Value]) -> Result<u64, Error> {
        Ok(0)
    }
    fn query(&mut self, sql: &str, _params: &[Value]) -> Result<Vec<DbRow>, Error> {
        if sql.contains("document_index WHERE") {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
        Ok(Vec::new())
    }
    fn query_once(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
        self.query(sql, params)
    }
    fn open_stream(&mut self, _sql: &str, _params: &[Value]) -> Result<(), Error> {
        Ok(())
    }
    fn next_row(&mut self) -> Result<Option<DbRow>, Error> {
        Ok(None)
    }
    fn close_stream(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn stream_open(&self) -> bool {
        false
    }
    fn begin(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn commit(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn rollback(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }
    fn rollback_to_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }
    fn release_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }
    fn canceller(&self) -> Box<dyn crate::driver::DbCancel> {
        Box::new(NoCancel)
    }
}

#[test]
fn a_ddl_lock_timeout_names_the_table() {
    let mut conn = RegistryConn(Arc::new(std::sync::atomic::AtomicUsize::new(0)));
    let err = super::ddl_transaction(&mut conn, "\"search\".\"t\"", |_| -> Result<(), Error> {
        Err(Error::new("postgres execute: canceling statement due to lock timeout"))
    })
    .unwrap_err();
    assert!(err.message().contains("\"search\".\"t\" stayed locked"), "{err}");
    let other = super::ddl_transaction(&mut conn, "t", |_| -> Result<(), Error> { Err(Error::new("syntax")) });
    assert_eq!(other.unwrap_err().message(), "syntax");
    assert_eq!(super::ddl_transaction(&mut conn, "t", |_| Ok(7)).unwrap(), 7);
}

#[test]
fn a_missing_table_is_remembered_until_ensure_or_drop() {
    let lookups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tx = TransactionManager::new(
        Arc::new(Registry(Arc::clone(&lookups))),
        crate::transaction::PoolConfig {
            min: 0,
            max: 2,
            reserve: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let backend = PostgresBackend::open(tx, None).unwrap();
    let party = mapping("MantleParty", "mantle_party");
    assert!(!backend.index_exists(&party).unwrap());
    assert!(!backend.index_exists(&party).unwrap());
    assert_eq!(lookups.load(Ordering::SeqCst), 1);
    backend.drop_index(&party).unwrap();
    assert!(!backend.index_exists(&party).unwrap());
    assert_eq!(lookups.load(Ordering::SeqCst), 2);
    // Past its time a missing table is looked up again, as another node may have made it.
    lock(&backend.absent).insert(party.document_id.clone(), Instant::now() - ABSENT_TTL);
    assert!(!backend.index_exists(&party).unwrap());
    assert_eq!(lookups.load(Ordering::SeqCst), 3);
}

#[test]
fn a_day_partitioned_table_conflicts_on_its_key_and_does_not_set_it() {
    let spans = crate::search::telemetry::telemetry_mapping("otel_spans").unwrap();
    let metrics = crate::search::telemetry::telemetry_mapping("otel_metrics").unwrap();
    let errors = crate::search::telemetry::telemetry_mapping("otel_errors").unwrap();
    assert!(
        partitions::Split::of(&errors).is_none(),
        "errors are upserted across days"
    );
    let split = partitions::Split::of(&spans).unwrap();
    assert_eq!(split.conflict_target(), r#"doc_id, "f_start""#);
    assert_eq!(
        partitions::Split::of(&metrics).unwrap().conflict_target(),
        r#"doc_id, "f_granularity", "f_bucket_start""#
    );
    let columns = insert_columns(&spans);
    let statement = insert_sql("t", &split.conflict_target(), &columns, 1, false);
    assert!(
        statement.contains(r#"ON CONFLICT (doc_id, "f_start") DO UPDATE"#),
        "{statement}"
    );
    assert!(!statement.contains(r#""f_start" = EXCLUDED"#), "{statement}");
    assert!(statement.contains(r#""f_name" = EXCLUDED."f_name""#), "{statement}");
}
