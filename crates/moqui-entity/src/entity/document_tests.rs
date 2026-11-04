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

use super::field_path_to_list;

#[test]
fn a_post_query_condition_compares_by_the_document_value_type() {
    use moqui_lib::object::Value;
    assert!(super::typed_equals(&Value::Decimal("5.00".parse().unwrap()), "5"));
    assert!(super::typed_equals(&Value::I64(7), "7"));
    assert!(!super::typed_equals(&Value::I64(7), "8"));
    assert!(super::typed_equals(&Value::Bool(true), "true"));
    assert!(super::typed_equals(&Value::string("Y"), "Y"));
}

#[test]
fn a_date_expression_returning_epoch_millis_is_a_timestamp() {
    use moqui_lib::object::Value;
    let field = |field_type: &str| super::DocField {
        path_element: "(x)".into(),
        alias: "x".into(),
        field_type: Some(field_type.into()),
        entity_type: None,
        sortable: false,
        function: None,
        is_expression: true,
        search_role: None,
        search_weight: None,
        default_display: None,
    };
    let stamp = super::expression_value(&field("date"), Value::string("1791979200000"));
    assert!(matches!(stamp, Value::Timestamp(_)), "{stamp:?}");
    assert!(matches!(
        super::expression_value(&field("date"), Value::F64(1791979200000.0)),
        Value::Timestamp(_)
    ));
    assert_eq!(
        super::expression_value(&field("double"), Value::string("12")),
        Value::string("12")
    );
}

#[test]
fn field_paths_keep_expression_colons() {
    assert_eq!(field_path_to_list("orderId"), vec!["orderId"]);
    assert_eq!(
        field_path_to_list("part:vendorRoles:roleTypeId"),
        vec!["part", "vendorRoles", "roleTypeId"]
    );
    assert_eq!(
        field_path_to_list("issuances:(acquireCost ? issuedQuantity * acquireCost : null)"),
        vec!["issuances", "(acquireCost ? issuedQuantity * acquireCost : null)"]
    );
    assert_eq!(field_path_to_list("(a ?: b)"), vec!["(a ?: b)"]);
}

// HL-001: child merging during assembly. A reference copy of the linear merge the index replaced: the documents
// assembled must be identical to what it makes.

use std::collections::BTreeMap;

use moqui_lib::object::Value;

use super::{DocField, Document, DocumentInfo, FieldNode, RelNode};

fn field(alias: &str) -> DocField {
    DocField {
        path_element: alias.into(),
        alias: alias.into(),
        field_type: None,
        entity_type: None,
        sortable: false,
        function: None,
        is_expression: false,
        search_role: None,
        search_weight: None,
        default_display: None,
    }
}

fn node(entity: &str, fields: Vec<DocField>, relationships: Vec<RelNode>) -> FieldNode {
    FieldNode {
        entity: entity.into(),
        member_alias: entity.into(),
        fields,
        relationships,
    }
}

fn many(alias: &str, node: FieldNode) -> RelNode {
    RelNode {
        relationship: alias.into(),
        document_alias: alias.into(),
        type_one: false,
        node,
    }
}

/// An order with items (each with notes, a type-one product, and an expression field) and a sibling list of payments.
fn order_info() -> DocumentInfo {
    let mut expression = field("(lineTotal)");
    expression.is_expression = true;
    let product = RelNode {
        relationship: "product".into(),
        document_alias: "product".into(),
        type_one: true,
        node: node("Product", vec![field("productName")], Vec::new()),
    };
    let items = many(
        "items",
        node(
            "OrderItem",
            vec![field("itemSeq"), field("productId"), expression],
            vec![
                many("notes", node("ItemNote", vec![field("note")], Vec::new())),
                product,
            ],
        ),
    );
    let payments = many("payments", node("Payment", vec![field("paymentId")], Vec::new()));
    DocumentInfo {
        data_document_id: "TestOrder".into(),
        index_name: None,
        primary_entity: "Order".into(),
        primary_entity_label: "Order".into(),
        primary_pk: vec!["orderId".into()],
        has_all_primary_pks: true,
        manual_data_service: None,
        manual_mapping_service: None,
        root: node("Order", vec![field("orderId")], vec![items, payments]),
        conditions: Vec::new(),
        has_expression: false,
    }
}

fn row(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let mut row: BTreeMap<String, Value> = pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect();
    if let Some(id) = row.get("orderId").cloned() {
        row.insert("_pk_orderId".into(), id);
    }
    row
}

fn text(value: &str) -> Value {
    Value::string(value)
}

/// The linear merge as it was before the index.
fn reference(info: &DocumentInfo, rows: &[BTreeMap<String, Value>]) -> Vec<Document> {
    fn set_fields(node: &FieldNode, row: &BTreeMap<String, Value>, target: &mut Document) {
        for field in node.fields.iter().filter(|field| !field.is_expression) {
            if let Some(value) = row.get(&field.alias).filter(|value| !value.is_null()) {
                target.insert(field.alias.clone(), value.clone());
            }
        }
    }
    fn populate(node: &FieldNode, row: &BTreeMap<String, Value>, target: &mut Document) {
        for rel in &node.relationships {
            if rel.type_one {
                set_fields(&rel.node, row, target);
                populate(&rel.node, row, target);
                continue;
            }
            let matched = match target.get_mut(&rel.document_alias) {
                Some(Value::List(list)) => list.iter_mut().find(|candidate| {
                    let Value::Map(candidate) = candidate else {
                        return false;
                    };
                    rel.node
                        .fields
                        .iter()
                        .filter(|field| !field.is_expression)
                        .all(|field| {
                            candidate.get(&field.alias).unwrap_or(&Value::Null)
                                == row.get(&field.alias).unwrap_or(&Value::Null)
                        })
                }),
                _ => None,
            };
            match matched {
                Some(Value::Map(existing)) => populate(&rel.node, row, existing),
                _ => {
                    let mut child = Document::new();
                    set_fields(&rel.node, row, &mut child);
                    populate(&rel.node, row, &mut child);
                    if child.is_empty() {
                        continue;
                    }
                    match target.get_mut(&rel.document_alias) {
                        Some(Value::List(list)) => list.push(Value::Map(child)),
                        _ => {
                            target.insert(rel.document_alias.clone(), Value::List(vec![Value::Map(child)]));
                        }
                    }
                }
            }
        }
    }
    let mut docs: indexmap::IndexMap<String, Document> = indexmap::IndexMap::new();
    for row in rows {
        let doc = docs.entry(info.doc_id(row)).or_insert_with(|| {
            let mut doc = Document::new();
            set_fields(&info.root, row, &mut doc);
            doc
        });
        populate(&info.root, row, doc);
    }
    docs.into_values().collect()
}

/// The assembled documents without the control fields the reference does not set.
fn assembled(info: &DocumentInfo, rows: &[BTreeMap<String, Value>]) -> Vec<Document> {
    let mut docs = super::merge_rows(info, rows, jiff::Timestamp::UNIX_EPOCH);
    for doc in &mut docs {
        for control in ["_type", "_id", "_timestamp", "_index", "_entity"] {
            doc.shift_remove(control);
        }
    }
    docs
}

fn list<'a>(doc: &'a Document, alias: &str) -> &'a Vec<Value> {
    match doc.get(alias) {
        Some(Value::List(list)) => list,
        other => panic!("{alias} is not a list: {other:?}"),
    }
}

fn map(value: &Value) -> &Document {
    match value {
        Value::Map(map) => map,
        other => panic!("not a map: {other:?}"),
    }
}

/// Duplicate children join the first match in insertion order; a null key and a missing one are the same; an
/// expression field is not compared; a row whose child has no values adds none; sibling lists merge apart.
#[test]
fn duplicate_children_and_null_keys_merge_as_before() {
    let info = order_info();
    let rows = vec![
        row(&[
            ("orderId", text("O1")),
            ("itemSeq", text("1")),
            ("productId", text("A")),
            ("productName", text("Apple")),
            ("note", text("n1")),
            ("paymentId", text("P1")),
            ("(lineTotal)", text("x")),
        ]),
        row(&[
            ("orderId", text("O1")),
            ("itemSeq", text("1")),
            ("productId", text("A")),
            ("note", text("n2")),
            ("paymentId", text("P1")),
            ("(lineTotal)", text("y")),
        ]),
        row(&[
            ("orderId", text("O1")),
            ("itemSeq", Value::Null),
            ("productId", text("B")),
            ("note", text("n3")),
            ("paymentId", text("P2")),
        ]),
        row(&[
            ("orderId", text("O1")),
            ("productId", text("B")),
            ("note", text("n4")),
            ("paymentId", text("P2")),
        ]),
        row(&[
            ("orderId", text("O1")),
            ("itemSeq", text("1")),
            ("productId", text("A")),
            ("note", text("n1")),
            ("paymentId", text("P2")),
        ]),
        row(&[
            ("orderId", text("O1")),
            ("itemSeq", Value::Null),
            ("productId", Value::Null),
            ("note", Value::Null),
            ("paymentId", Value::Null),
        ]),
        row(&[
            ("orderId", text("O1")),
            ("itemSeq", Value::I64(1)),
            ("productId", text("A")),
            ("note", text("typed")),
        ]),
        row(&[
            ("orderId", text("O2")),
            ("itemSeq", text("1")),
            ("productId", text("A")),
            ("note", text("other order")),
        ]),
    ];
    let docs = assembled(&info, &rows);
    assert_eq!(docs, reference(&info, &rows));
    assert_eq!(docs.len(), 2);
    let items = list(&docs[0], "items");
    // Item 1/A, item null/B, and item 1 (a number, not equal to the text "1")/A.
    assert_eq!(items.len(), 3, "{items:?}");
    assert_eq!(map(&items[0]).get("productName"), Some(&text("Apple")));
    assert_eq!(list(map(&items[0]), "notes").len(), 2, "n1 joined once, n2 added");
    assert_eq!(
        list(map(&items[1]), "notes").len(),
        2,
        "the null and the missing item key are one item"
    );
    assert_eq!(list(&docs[0], "payments").len(), 2);
}

/// One large child list, and two sibling to-many lists whose join multiplies the rows: identical to the linear merge.
#[test]
fn a_large_child_list_and_sibling_lists_merge_as_before() {
    let info = order_info();
    let mut rows = Vec::new();
    for item in 0..300 {
        for payment in 0..3 {
            rows.push(row(&[
                ("orderId", text("O1")),
                ("itemSeq", text(&format!("{item:05}"))),
                ("productId", text(&format!("P{}", item % 7))),
                ("note", text(&format!("note {}", item % 2))),
                ("paymentId", text(&format!("PAY{payment}"))),
            ]));
        }
    }
    // Rows arrive again out of order: they join the children already there.
    let again: Vec<_> = rows.iter().rev().step_by(5).cloned().collect();
    rows.extend(again);
    let docs = assembled(&info, &rows);
    assert_eq!(docs, reference(&info, &rows));
    assert_eq!(list(&docs[0], "items").len(), 300);
    assert_eq!(list(&docs[0], "payments").len(), 3);
}

thread_local! {
    /// Children compared with a row during assembly on this thread, for the measurement below.
    pub(super) static COMPARED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// HL-001 measurement, opt-in: comparisons and time to assemble one parent with N distinct children, against the
/// linear merge (whose comparisons are about N²/2). `cargo test -p moqui-entity --lib child_merge_cost -- --ignored
/// --nocapture`.
#[test]
#[ignore = "measurement: prints child comparisons and assembly time for large child lists"]
fn child_merge_cost() {
    let info = order_info();
    for children in [1_000, 4_000, 8_000] {
        let rows: Vec<_> = (0..children)
            .map(|item| {
                row(&[
                    ("orderId", text("O1")),
                    ("itemSeq", text(&format!("{item:06}"))),
                    ("productId", text("A")),
                ])
            })
            .collect();
        COMPARED.with(|compared| compared.set(0));
        let started = std::time::Instant::now();
        let docs = assembled(&info, &rows);
        let indexed = started.elapsed();
        let compared = COMPARED.with(std::cell::Cell::get);
        let started = std::time::Instant::now();
        let linear = reference(&info, &rows);
        let linear_time = started.elapsed();
        assert_eq!(docs, linear);
        println!(
            "{children} children: indexed {compared} comparisons in {indexed:?}; linear about {} comparisons in \
             {linear_time:?}",
            children * (children - 1) / 2
        );
    }
}
