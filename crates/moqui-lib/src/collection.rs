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

//! Map and list helpers from `CollectionUtilities`.
//!
//! Only the helpers the framework calls are ported.

use indexmap::IndexMap;

use crate::error::Error;
use crate::object::{is_empty, to_plain_string, Value};

/// The `key` value of each map. A missing key is a null, as in Java, so the values line up
/// with the maps unless `exclude_null` drops them.
pub fn get_map_list_values(maps: &[crate::object::ValueMap], key: &str, exclude_null: bool) -> Vec<Value> {
    let mut values = Vec::with_capacity(maps.len());
    for map in maps {
        let value = map.get(key).unwrap_or(&Value::Null);
        if exclude_null && value.is_null() {
            continue;
        }
        values.push(value.clone());
    }
    values
}

pub fn flatten_nested_map(map: &crate::object::ValueMap) -> crate::object::ValueMap {
    let mut out = IndexMap::default();
    flatten_into(map, &mut out);
    out
}

fn flatten_into(map: &crate::object::ValueMap, out: &mut crate::object::ValueMap) {
    for (key, value) in map {
        match value {
            Value::Map(nested) => flatten_into(nested, out),
            Value::List(items) => {
                for item in items {
                    if let Value::Map(nested) = item {
                        flatten_into(nested, out);
                    }
                }
            }
            other => {
                out.insert(key.clone(), other.clone());
            }
        }
    }
}

/// Page keys are `{name}Count`, `PageIndex`, `PageSize`, `PageMaxIndex`,
/// `PageRangeLow`, and `PageRangeHigh`. `pageIndex` defaults to 0 and `pageSize`
/// to 20. When `{name}Count` is already set, this only sets `{name}AlreadyPaginated`.
pub fn paginate_parameters(
    list_size: i32,
    page_list_name: &str,
    context: &mut crate::object::ValueMap,
) -> Result<(), Error> {
    let mut page_index = context_int(context, "pageIndex").unwrap_or(0);
    if page_index < 0 {
        page_index = 0;
    }
    let mut page_size = context_int(context, "pageSize").unwrap_or(20);
    if page_size < 0 {
        page_size = 20;
    }
    if page_size == 0 {
        return Err(Error::new("pageSize is 0"));
    }
    let count_key = format!("{page_list_name}Count");
    if context.get(&count_key).is_some_and(|value| !value.is_null()) {
        context.insert(format!("{page_list_name}AlreadyPaginated"), Value::Bool(true));
        return Ok(());
    }
    let count = list_size.max(0);
    let max_index = if count == 0 { 0 } else { (count - 1) / page_size };
    let mut page_range_low = page_index
        .checked_mul(page_size)
        .and_then(|start| start.checked_add(1))
        .ok_or_else(|| Error::new("page index overflow"))?;
    if page_range_low > count {
        page_range_low = count + 1;
    }
    let mut page_range_high = page_index
        .checked_mul(page_size)
        .and_then(|start| start.checked_add(page_size))
        .ok_or_else(|| Error::new("page index overflow"))?;
    if page_range_high > count {
        page_range_high = count;
    }
    context.insert(count_key, Value::I32(count));
    context.insert(format!("{page_list_name}PageIndex"), Value::I32(page_index));
    context.insert(format!("{page_list_name}PageSize"), Value::I32(page_size));
    context.insert(format!("{page_list_name}PageMaxIndex"), Value::I32(max_index));
    context.insert(format!("{page_list_name}PageRangeLow"), Value::I32(page_range_low));
    context.insert(format!("{page_list_name}PageRangeHigh"), Value::I32(page_range_high));
    Ok(())
}

fn context_int(context: &crate::object::ValueMap, key: &str) -> Option<i32> {
    let value = context.get(key)?;
    if is_empty(value) {
        return None;
    }
    match value {
        Value::I32(number) => Some(*number),
        Value::I64(number) => i32::try_from(*number).ok(),
        Value::Decimal(number) => i32::try_from(number.mantissa()).ok().filter(|_| number.scale() == 0),
        Value::String(text) => text.parse().ok(),
        _ => to_plain_string(value).parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::java_equals;

    fn row(pairs: &[(&str, Value)]) -> crate::object::ValueMap {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn map_list_values_keep_alignment() {
        let maps = vec![
            row(&[("a", Value::I32(1))]),
            row(&[("b", Value::I32(2))]),
            row(&[("a", Value::Null)]),
            row(&[("a", Value::I32(3))]),
        ];
        let all = get_map_list_values(&maps, "a", false);
        assert_eq!(all.len(), 4);
        assert!(all[1].is_null() && all[2].is_null());
        let present = get_map_list_values(&maps, "a", true);
        assert_eq!(present.len(), 2);
        assert!(java_equals(&present[1], &Value::I32(3)));
    }

    #[test]
    fn nested_maps_flatten() {
        let mut nested = IndexMap::default();
        nested.insert("id".to_string(), Value::string("2"));
        let mut root = IndexMap::default();
        root.insert("child".to_string(), Value::Map(nested));
        let flat = flatten_nested_map(&root);
        assert_eq!(flat.get("id"), Some(&Value::string("2")));
    }

    #[test]
    fn pagination_sets_the_page_keys() {
        let mut context = IndexMap::default();
        context.insert("pageIndex".to_string(), Value::I32(1));
        context.insert("pageSize".to_string(), Value::I32(20));
        paginate_parameters(25, "rows", &mut context).unwrap();
        assert_eq!(context.get("rowsPageRangeLow"), Some(&Value::I32(21)));
        assert_eq!(context.get("rowsPageRangeHigh"), Some(&Value::I32(25)));
        assert_eq!(context.get("rowsPageMaxIndex"), Some(&Value::I32(1)));
        paginate_parameters(25, "rows", &mut context).unwrap();
        assert_eq!(context.get("rowsAlreadyPaginated"), Some(&Value::Bool(true)));
    }
}
