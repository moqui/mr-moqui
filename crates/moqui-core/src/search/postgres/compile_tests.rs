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
use crate::search::mapping::ObjectMapping;
use crate::search::parse::parse_query_string;

fn party() -> DocumentMapping {
    let field = |kind| FieldMapping { kind, sortable: false };
    let mut root = ObjectMapping::default();
    root.fields.insert("partyId".into(), field(FieldKind::Keyword));
    root.fields.insert("combinedName".into(), field(FieldKind::Text));
    root.fields.insert("placedDate".into(), field(FieldKind::Date));
    let mut classes = ObjectMapping::default();
    classes
        .fields
        .insert("partyClassificationId".into(), field(FieldKind::Keyword));
    classes.fields.insert("classFromDate".into(), field(FieldKind::Date));
    classes.fields.insert("classThruDate".into(), field(FieldKind::Date));
    root.children.insert("classifications".into(), classes);
    let mut roles = ObjectMapping::default();
    roles.fields.insert("roleTypeId".into(), field(FieldKind::Keyword));
    root.children.insert("roles".into(), roles);
    DocumentMapping {
        document_id: "MantleParty".into(),
        index_name: Some("mantle".into()),
        physical_name: "mantle_party".into(),
        root,
        manual_mapping_service: None,
        display: Default::default(),
    }
}

#[test]
fn a_monitor_lucene_string_compiles_as_the_facade_grammar() {
    let mapping = crate::search::telemetry::telemetry_mapping("otel_logs").unwrap();
    let text = "timestamp:[1760000000000 TO 1760600000000] AND (severity_text:WARN OR \"slow statement\")";
    let lucene = Compiler::new(&mapping).where_sql(&Query::Lucene(text.into())).unwrap();
    let parsed = Compiler::new(&mapping)
        .where_sql(&parse_query_string(text).unwrap())
        .unwrap();
    assert_eq!(lucene.text, parsed.text);
    assert!(lucene.text.contains("f_timestamp"), "{}", lucene.text);
    assert!(Compiler::new(&mapping)
        .where_sql(&Query::Lucene("slow~".into()))
        .is_ok());
}

#[test]
fn two_near_words_are_phrase_operators_and_more_read_positions() {
    let mapping = party();
    let near = |terms: &str| {
        Query::from_value(&moqui_lib::json::from_json(
            &serde_json::json!({"near": {"terms": terms, "fields": ["combinedName"], "distance": 2}}),
        ))
        .unwrap()
    };
    let sql = Compiler::new(&mapping).where_sql(&near("water pump")).unwrap();
    assert!(!sql.text.contains("unnest"), "{}", sql.text);
    assert!(
        sql.params
            .contains(&Value::string("water <1> pump | water <2> pump | water <3> pump")),
        "{:?}",
        sql.params
    );
    let sql = Compiler::new(&mapping).where_sql(&near("big water pump")).unwrap();
    assert!(sql.text.contains("unnest"), "{}", sql.text);
    // The second and third words are each held to the window from the first as the join adds them.
    let windows: Vec<&str> = sql
        .text
        .match_indices(" <= ")
        .map(|(at, _)| &sql.text[at..at + 5])
        .collect();
    assert_eq!(windows, vec![" <= 3", " <= 4"], "{}", sql.text);
}

#[test]
fn dotted_field_is_any_element() {
    let mapping = party();
    let sql = Compiler::new(&mapping)
        .where_sql(&parse_query_string("roles.roleTypeId:Customer").unwrap())
        .unwrap();
    assert_eq!(
        sql.text,
        "EXISTS (SELECT 1 FROM jsonb_array_elements(body -> 'roles') AS e0 WHERE COALESCE((e0 ->> 'roleTypeId') = ?, FALSE))"
    );
    assert_eq!(sql.params, vec![Value::string("Customer")]);
}

#[test]
fn child_query_binds_one_element() {
    let mapping = party();
    let query = Query::child(
        "classifications",
        Query::Bool {
            all: vec![
                Query::eq("classifications.partyClassificationId", Value::string("CustVip")),
                Query::range_full("classifications.classFromDate", None, None, None, Some(Value::I64(0))),
            ],
            any: vec![
                Query::range("classifications.classThruDate", Some(Value::I64(0)), None),
                Query::exclude(Query::exists("classifications.classThruDate")),
            ],
            none: vec![],
            min_any: Some(1),
        },
    );
    let sql = Compiler::new(&mapping).where_sql(&query).unwrap();
    assert!(sql
        .text
        .starts_with("EXISTS (SELECT 1 FROM jsonb_array_elements(body -> 'classifications') AS e0 WHERE ("));
    assert!(
        sql.text
            .contains("COALESCE((e0 ->> 'partyClassificationId') = ?, FALSE)"),
        "{}",
        sql.text
    );
    assert!(sql.text.contains(" OR (NOT COALESCE("), "{}", sql.text);
    // A child date is read from its text: digits as epoch millis, a bare date as midnight UTC, else an ISO time.
    assert!(
        sql.text
            .contains(&format!("{} >= ?::timestamptz", ts_expr("(e0 ->> 'classThruDate')"))),
        "{}",
        sql.text
    );
    assert_eq!(sql.params.len(), 3);
}

#[test]
fn a_child_date_text_is_epoch_millis_a_bare_date_or_a_time() {
    assert_eq!(
        ts_expr("(e0 ->> 'd')"),
        "(CASE WHEN (e0 ->> 'd') ~ '^-?[0-9]+$' THEN to_timestamp(((e0 ->> 'd'))::bigint / 1000.0) \
             WHEN length((e0 ->> 'd')) = 10 THEN ((e0 ->> 'd') || 'T00:00:00Z')::timestamptz \
             ELSE ((e0 ->> 'd'))::timestamptz END)"
    );
}

#[test]
fn text_kinds_and_missing_fields() {
    let mapping = party();
    let compiler = Compiler::new(&mapping);
    let sql = compiler.where_sql(&parse_query_string("*:jo*").unwrap()).unwrap();
    assert_eq!(sql.text, "all_text @@ to_tsquery('simple', ?)");
    assert_eq!(sql.params, vec![Value::string("jo:*")]);
    let sql = compiler
        .where_sql(&parse_query_string("combinedName:*mit*").unwrap())
        .unwrap();
    assert_eq!(sql.text, "COALESCE(\"f_combinedName\" ILIKE ?, FALSE)");
    let sql = compiler.where_sql(&parse_query_string("partyId:AB*").unwrap()).unwrap();
    assert_eq!(sql.text, "COALESCE(\"f_partyId\" LIKE ?, FALSE)");
    assert_eq!(sql.params, vec![Value::string("AB%")]);
    let sql = compiler.where_sql(&parse_query_string("nothere:x").unwrap()).unwrap();
    assert_eq!(sql.text, "FALSE");
    let sql = compiler
        .where_sql(&parse_query_string("placedDate:2026-01-05").unwrap())
        .unwrap();
    assert!(sql.text.contains(">= ?::timestamptz AND"), "{}", sql.text);
}

#[test]
fn a_term_on_a_root_keyword_field_also_matches_an_element_of_a_list_there() {
    let mapping = party();
    let sql = Compiler::new(&mapping)
        .where_sql(&Query::eq("partyId", Value::string("T1")))
        .unwrap();
    assert_eq!(
        sql.text,
        "COALESCE(COALESCE(\"f_partyId\" = ?, (body -> 'partyId') @> jsonb_build_array(?::text)), FALSE)"
    );
    assert_eq!(sql.params, vec![Value::string("T1"), Value::string("T1")]);
    let sql = Compiler::new(&mapping)
        .where_sql(&Query::is_in("partyId", vec![Value::string("A"), Value::string("B")]))
        .unwrap();
    assert_eq!(
        sql.text,
        "COALESCE(COALESCE(\"f_partyId\" IN (?, ?), (body -> 'partyId') @> jsonb_build_array(?::text) OR \
             (body -> 'partyId') @> jsonb_build_array(?::text)), FALSE)"
    );
    assert_eq!(sql.params.len(), 4);
    // a dotted field is already any element of its child array; a range is not a term
    let sql = Compiler::new(&mapping)
        .where_sql(&Query::eq("roles.roleTypeId", Value::string("Customer")))
        .unwrap();
    assert!(!sql.text.contains("@>"), "{}", sql.text);
}

#[test]
fn should_only_bool_needs_one() {
    let mapping = party();
    let sql = Compiler::new(&mapping)
        .where_sql(&parse_query_string("partyId:(A OR B)").unwrap())
        .unwrap();
    assert_eq!(
        sql.text,
        "((COALESCE(\"f_partyId\" = ?, FALSE) OR COALESCE(\"f_partyId\" = ?, FALSE)))"
    );
}

#[test]
fn a_relative_date_range_resolves_as_opensearch_date_math() {
    let mapping = party();
    let sql = Compiler::new(&mapping)
        .where_sql(&Query::range(
            "placedDate",
            Some(Value::string("now-30d")),
            Some(Value::string("now")),
        ))
        .unwrap();
    assert_eq!(sql.params.len(), 2, "{}", sql.text);
    let (Value::Timestamp(from), Value::Timestamp(thru)) = (&sql.params[0], &sql.params[1]) else {
        panic!("{:?}", sql.params)
    };
    let width = thru.as_millisecond() - from.as_millisecond();
    // Thirty calendar days, which a daylight-saving change may lengthen or shorten by an hour.
    assert!((width - 30 * 86_400_000).abs() <= 3_600_000, "{width}");
    // An `lte` rounded bound is the last millisecond of its unit, a `gte` one the first.
    let sql = Compiler::new(&mapping)
        .where_sql(&Query::range_full(
            "placedDate",
            None,
            Some(Value::string("now-1d/d")),
            None,
            Some(Value::string("now-1d/d")),
        ))
        .unwrap();
    let (Value::Timestamp(start), Value::Timestamp(end)) = (&sql.params[0], &sql.params[1]) else {
        panic!("{:?}", sql.params)
    };
    let day = end.as_millisecond() + 1 - start.as_millisecond();
    assert!((day - 86_400_000).abs() <= 3_600_000, "{day}");
    assert!(parse_date_bound(&Value::string("now-bad")).is_none());
}
