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
use crate::search::mapping::{FieldMapping, ObjectMapping};

fn sales() -> DocumentMapping {
    let field = |kind| FieldMapping { kind, sortable: false };
    let mut root = ObjectMapping::default();
    for (name, kind) in [
        ("orderId", FieldKind::Keyword),
        ("productId", FieldKind::Keyword),
        ("itemTypeEnumId", FieldKind::Keyword),
        ("placedDate", FieldKind::Date),
        ("itemSaleTotal", FieldKind::Double),
    ] {
        root.fields.insert(name.into(), field(kind));
    }
    let mut issuances = ObjectMapping::default();
    issuances
        .fields
        .insert("issuedSaleTotal".into(), field(FieldKind::Double));
    root.children.insert("issuances".into(), issuances);
    DocumentMapping {
        document_id: "MantleSalesOrderItem".into(),
        index_name: Some("mantle_sales".into()),
        physical_name: "mantle_sales_order_item".into(),
        root,
        manual_mapping_service: None,
        display: Default::default(),
    }
}

fn tree(text: &str) -> IndexMap<String, Agg> {
    Agg::map_from_value(&moqui_lib::json::from_json(&serde_json::from_str(text).unwrap())).unwrap()
}

#[test]
fn levels_group_filter_and_lateral_sum() {
    let aggs = tree(
        r#"{"DateOuter":{"date_histogram":{"field":"placedDate","unit":"week","time_zone":"UTC","offset":"-1d"},
                "aggs":{"orderCount":{"count_distinct":{"field":"orderId"}},
                        "ItemProduct":{"filter":{"term":{"itemTypeEnumId":"ItemProduct"}},
                            "aggs":{"productIssuedTotal":{"sum":{"field":"issuances.issuedSaleTotal"}}}}}}}"#,
    );
    let mut statements = Vec::new();
    let mut run = |sql: &Sql| -> Result<Vec<DbRow>, Error> {
        statements.push(sql.clone());
        Ok(Vec::new())
    };
    let out = aggregate(
        "\"search\".\"t\"",
        &sales(),
        &TextSetup::default(),
        &Query::MatchAll,
        &aggs,
        &mut run,
    )
    .unwrap();
    assert_eq!(statements.len(), 2);
    assert_eq!(
        statements[0].text,
        "SELECT count(*) AS dc FROM \"search\".\"t\" WHERE TRUE"
    );
    let level = &statements[1].text;
    // JSON object keys come back sorted, so ItemProduct is collected before orderCount.
    assert!(level.starts_with(
        "SELECT ((date_trunc('week', (\"f_placedDate\" AT TIME ZONE 'UTC') - interval '-86400000 milliseconds') + interval '-86400000 milliseconds') AT TIME ZONE 'UTC') AS k0, count(*) AS dc, "
    ), "{level}");
    assert!(
        level.contains(
            "count(*) FILTER (WHERE COALESCE(COALESCE(\"f_itemTypeEnumId\" = ?, (body -> 'itemTypeEnumId') @> jsonb_build_array(?::text)), FALSE)) AS c0"
        ),
        "{level}"
    );
    assert!(
        level.contains(
            "COALESCE(sum(s1.v) FILTER (WHERE COALESCE(COALESCE(\"f_itemTypeEnumId\" = ?, (body -> 'itemTypeEnumId') @> jsonb_build_array(?::text)), FALSE)), 0) AS c1"
        ),
        "{level}"
    );
    assert!(level.contains("count(DISTINCT \"f_orderId\") AS c2"), "{level}");
    assert!(level.contains("LEFT JOIN LATERAL (SELECT sum((e"), "{level}");
    assert!(level.ends_with("GROUP BY 1 LIMIT 65537"), "{level}");
    let Value::Map(outer) = &out["DateOuter"] else { panic!() };
    assert_eq!(outer["buckets"], Value::List(Vec::new()));
}

#[test]
fn min_and_max_levels_filter_and_read_null() {
    let aggs = tree(
        r#"{"hi":{"max":{"field":"issuances.issuedSaleTotal"}},
                "lo":{"filter":{"term":{"itemTypeEnumId":"ItemProduct"}},"aggs":{"m":{"min":{"field":"itemSaleTotal"}}}}}"#,
    );
    let mut statements = Vec::new();
    let mut run = |sql: &Sql| -> Result<Vec<DbRow>, Error> {
        statements.push(sql.clone());
        Ok(vec![DbRow {
            columns: vec![("dc".into(), Value::I64(0))],
        }])
    };
    let out = aggregate(
        "\"search\".\"t\"",
        &sales(),
        &TextSetup::default(),
        &Query::MatchAll,
        &aggs,
        &mut run,
    )
    .unwrap();
    let level = &statements.last().unwrap().text;
    assert!(level.contains("max(s0.v) AS c"), "{level}");
    assert!(level.contains("LEFT JOIN LATERAL (SELECT max("), "{level}");
    assert!(level.contains("min(\"f_itemSaleTotal\") FILTER (WHERE"), "{level}");
    assert_eq!(out["hi"], Value::Null);
}

#[test]
fn a_level_past_the_group_cap_is_an_error() {
    let aggs = tree(r#"{"P":{"terms":{"field":"productId","size":10}}}"#);
    let mut run = |sql: &Sql| -> Result<Vec<DbRow>, Error> {
        if !sql.text.contains("GROUP BY") {
            return Ok(vec![DbRow {
                columns: vec![("dc".into(), Value::I64(1))],
            }]);
        }
        // What the database returns for the LIMIT when there are more groups than that.
        let limit: usize = sql.text.rsplit(' ').next().unwrap().parse().unwrap();
        assert_eq!(limit, MAX_GROUPS + 1);
        let row = DbRow {
            columns: vec![("k0".into(), Value::string("p")), ("dc".into(), Value::I64(1))],
        };
        Ok(vec![row; limit])
    };
    let err = aggregate(
        "\"search\".\"t\"",
        &sales(),
        &TextSetup::default(),
        &Query::MatchAll,
        &aggs,
        &mut run,
    )
    .unwrap_err();
    assert!(err.message().contains("more than 65536 groups"), "{err}");
    assert!(crate::search::limits::is_too_many_groups(&err));
}

#[test]
fn gaps_fill_between_first_and_last() {
    let aggs = tree(
        r#"{"D":{"date_histogram":{"field":"placedDate","unit":"day","time_zone":"UTC","order":"desc","format":"dd MMM yyyy"},"aggs":{"t":{"sum":{"field":"itemSaleTotal"}}}}}"#,
    );
    let Agg::DateHistogram(histogram) = &aggs["D"] else {
        panic!()
    };
    let day = |text: &str| Value::Timestamp(text.parse().unwrap());
    let mut first = IndexMap::new();
    first.insert("doc_count".to_string(), Value::I64(2));
    let mut third = IndexMap::new();
    third.insert("doc_count".to_string(), Value::I64(1));
    let buckets = histogram_buckets(
        histogram,
        vec![
            (day("2026-01-03T00:00:00Z"), third),
            (day("2026-01-01T00:00:00Z"), first),
        ],
    )
    .unwrap();
    let shown: Vec<(String, i64)> = buckets
        .iter()
        .map(|bucket| {
            let Value::Map(bucket) = bucket else { panic!() };
            let Value::I64(count) = bucket["doc_count"] else {
                panic!()
            };
            (to_plain_string(&bucket["key_display"]), count)
        })
        .collect();
    assert_eq!(
        shown,
        vec![
            ("03 Jan 2026".into(), 1),
            ("02 Jan 2026".into(), 0),
            ("01 Jan 2026".into(), 2)
        ]
    );
    let Value::Map(gap) = &buckets[1] else { panic!() };
    assert!(matches!(gap["t"], Value::Decimal(_)));
}

#[test]
fn a_fixed_offset_zone_is_an_interval_and_buckets_at_its_midnight() {
    let aggs =
        tree(r#"{"D":{"date_histogram":{"field":"placedDate","unit":"day","time_zone":"+05:00","offset":"500ms"}}}"#);
    let mut statements = Vec::new();
    let mut run = |sql: &Sql| -> Result<Vec<DbRow>, Error> {
        statements.push(sql.text.clone());
        Ok(Vec::new())
    };
    aggregate(
        "\"search\".\"t\"",
        &sales(),
        &TextSetup::default(),
        &Query::MatchAll,
        &aggs,
        &mut run,
    )
    .unwrap();
    assert!(statements[1].starts_with(
        "SELECT ((date_trunc('day', (\"f_placedDate\" AT TIME ZONE interval '18000 seconds') - interval '500 milliseconds') + interval '500 milliseconds') AT TIME ZONE interval '18000 seconds') AS k0"
    ), "{}", statements[1]);
    assert_eq!(
        zone_sql(&TimeZone::fixed(jiff::tz::offset(-5))).unwrap(),
        "interval '-18000 seconds'"
    );
    assert_eq!(
        zone_sql(&TimeZone::get("Asia/Karachi").unwrap()).unwrap(),
        "'Asia/Karachi'"
    );

    // The keys Postgres makes, local midnight at +05:00, match the gap filling's own buckets.
    let Agg::DateHistogram(histogram) = &aggs["D"] else {
        panic!()
    };
    let key = |text: &str| Value::Timestamp(text.parse().unwrap());
    let count = |n: i64| IndexMap::from([("doc_count".to_string(), Value::I64(n))]);
    let buckets = histogram_buckets(
        histogram,
        vec![
            (key("2026-01-01T19:00:00.5Z"), count(2)),
            (key("2026-01-03T19:00:00.5Z"), count(1)),
        ],
    )
    .unwrap();
    let keys: Vec<(String, i64)> = buckets
        .iter()
        .map(|bucket| {
            let Value::Map(bucket) = bucket else { panic!() };
            let Value::I64(count) = bucket["doc_count"] else {
                panic!()
            };
            (to_plain_string(&bucket["key"]), count)
        })
        .collect();
    assert_eq!(keys.len(), 3, "{keys:?}");
    assert_eq!(keys[1].1, 0);
    assert_eq!(
        buckets
            .iter()
            .map(|bucket| {
                let Value::Map(bucket) = bucket else { panic!() };
                timestamp_of(&bucket["key"]).unwrap().to_string()
            })
            .collect::<Vec<_>>(),
        [
            "2026-01-01T19:00:00.5Z",
            "2026-01-02T19:00:00.5Z",
            "2026-01-03T19:00:00.5Z"
        ]
    );
}

#[test]
fn terms_order_by_metric_then_key_and_trim() {
    let terms = Terms {
        field: "productId".into(),
        size: 2,
        order: TermsOrder::Metric {
            name: "totalSales".into(),
            descending: true,
        },
        aggs: IndexMap::new(),
        missing: None,
        other: None,
        min_doc_count: 1,
    };
    let row = |key: &str, total: i64| {
        let mut content = IndexMap::new();
        content.insert("doc_count".to_string(), Value::I64(1));
        content.insert("totalSales".to_string(), Value::I64(total));
        (Value::string(key), content)
    };
    let buckets = terms_buckets(&terms, vec![row("B", 5), row("A", 5), row("C", 9)]);
    let keys: Vec<String> = buckets
        .iter()
        .map(|bucket| {
            let Value::Map(bucket) = bucket else { panic!() };
            to_plain_string(&bucket["key"])
        })
        .collect();
    assert_eq!(keys, vec!["C", "A"]);
}
