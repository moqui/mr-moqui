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

//! Columns a find reads and their Java types. `moqui_db` binds the condition values itself.

use std::collections::BTreeMap;

use moqui_db::model::{EntityDefinition, EntityModel, FieldDefinition};
use moqui_lib::object::{basic_convert, Value};

use crate::driver::DbRow;

/// Java type name for a dictionary field, when the dictionary has that type.
pub(crate) fn java_of(model: &EntityModel, field: &FieldDefinition) -> Option<String> {
    model
        .dictionary_type(&field.field_type)
        .map(|entry| entry.java_type.clone())
}

pub(crate) fn entity_columns(entity: &EntityDefinition, model: &EntityModel) -> Vec<(String, String)> {
    entity
        .fields
        .iter()
        .map(|field| (field.name.clone(), java_of(model, field).unwrap_or_default()))
        .collect()
}

/// Output column name to Java type for a view. Function aliases stay unconverted.
pub(crate) fn view_columns(view: &EntityDefinition) -> Vec<(String, String)> {
    view.aliases
        .iter()
        .map(|alias| {
            let java = if alias.function.is_some() {
                String::new()
            } else {
                alias.java_type.clone()
            };
            (alias.name.clone(), java)
        })
        .collect()
}

pub(crate) fn map_row(columns: &[(String, String)], row: &DbRow) -> BTreeMap<String, Value> {
    let mut values = BTreeMap::new();
    if columns.is_empty() {
        for (name, value) in &row.columns {
            values.insert(name.clone(), value.clone());
        }
        return values;
    }
    for (name, java) in columns {
        let raw = row.get(name).cloned().unwrap_or(Value::Null);
        let value = if java.is_empty() {
            raw
        } else {
            strip_zeros(basic_convert(&raw, java).value)
        };
        values.insert(name.clone(), value);
    }
    for (name, value) in &row.columns {
        values.entry(name.clone()).or_insert_with(|| value.clone());
    }
    values
}

/// Java's `FieldInfo.safeStripZeroes`: a decimal read for an entity field drops the column's
/// trailing zeros (1.000000 reads as 1), so only computed values carry a scale.
fn strip_zeros(value: Value) -> Value {
    match value {
        Value::Decimal(number) => Value::Decimal(number.normalize()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::map_row;
    use crate::driver::DbRow;
    use moqui_lib::object::{to_plain_string, Value};

    #[test]
    fn a_decimal_field_reads_without_the_column_scale() {
        let decimal = |text: &str| Value::Decimal(text.parse().unwrap());
        let row = DbRow {
            columns: vec![
                ("quantity".into(), decimal("1.000000")),
                ("amount".into(), decimal("16.99000")),
                ("total".into(), decimal("100.000000")),
                ("raw".into(), decimal("2.50")),
            ],
        };
        let columns = [
            ("quantity".to_string(), "java.math.BigDecimal".to_string()),
            ("amount".to_string(), "java.math.BigDecimal".to_string()),
            ("total".to_string(), "java.math.BigDecimal".to_string()),
            ("raw".to_string(), String::new()),
        ];
        let values = map_row(&columns, &row);
        let text = |name: &str| to_plain_string(&values[name]);
        assert_eq!(text("quantity"), "1");
        assert_eq!(text("amount"), "16.99");
        // As Java's safeStripZeroes, 1E+2 is set back to scale 0.
        assert_eq!(text("total"), "100");
        assert_eq!(text("raw"), "2.50");
    }
}
