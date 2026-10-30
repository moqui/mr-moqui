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
