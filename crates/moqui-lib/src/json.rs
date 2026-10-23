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

//! [`Value`] to and from JSON. Whole numbers stay integers and other numbers stay decimals, as
//! Groovy's `JsonSlurper` reads them, so a price does not pick up binary rounding.

use std::str::FromStr;

use rust_decimal::Decimal;

use crate::object::{basic_convert, to_plain_string, Value};

/// JSON for a value. Timestamps, dates, and times are their text; bytes are base64-free and
/// become null, since no service sends them over JSON.
pub fn to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Null | Value::Bytes(_) | Value::Object(_) => serde_json::Value::Null,
        Value::Bool(flag) => serde_json::Value::Bool(*flag),
        Value::I32(number) => serde_json::Value::from(*number),
        Value::I64(number) => serde_json::Value::from(*number),
        Value::F64(number) => serde_json::Number::from_f64(*number).map_or(serde_json::Value::Null, Into::into),
        Value::Decimal(number) => serde_json::Number::from_str(&number.to_string()).map_or_else(
            |_| serde_json::Value::String(number.to_string()),
            serde_json::Value::Number,
        ),
        Value::String(text) => serde_json::Value::String(text.clone()),
        Value::Timestamp(_) | Value::Date(_) | Value::Time(_) => {
            let shown = basic_convert(value, "String").value;
            serde_json::Value::String(shown.as_str().map_or_else(|| to_plain_string(value), str::to_string))
        }
        Value::List(items) => serde_json::Value::Array(items.iter().map(to_json).collect()),
        Value::Map(entries) => {
            serde_json::Value::Object(entries.iter().map(|(key, item)| (key.clone(), to_json(item))).collect())
        }
    }
}

/// JSON text for a value, with each map's keys in the map's order, as Groovy's `JsonOutput` writes a
/// `LinkedHashMap`. [`to_json`] sorts them: a `serde_json` object is a `BTreeMap`.
pub fn to_json_text(value: &Value) -> String {
    let mut out = String::new();
    write_json_text(&mut out, value);
    out
}

fn write_json_text(out: &mut String, value: &Value) {
    match value {
        Value::List(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_json_text(out, item);
            }
            out.push(']');
        }
        Value::Map(entries) => {
            out.push('{');
            for (index, (key, item)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::Value::String(key.clone()).to_string());
                out.push(':');
                write_json_text(out, item);
            }
            out.push('}');
        }
        leaf => out.push_str(&to_json(leaf).to_string()),
    }
}

/// A value for JSON. Objects keep their key order.
pub fn from_json(json: &serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(flag) => Value::Bool(*flag),
        serde_json::Value::String(text) => Value::String(text.clone()),
        serde_json::Value::Number(number) => {
            if let Some(whole) = number.as_i64() {
                return Value::I64(whole);
            }
            let text = number.to_string();
            match Decimal::from_str(&text).or_else(|_| Decimal::from_scientific(&text)) {
                Ok(decimal) => Value::Decimal(decimal),
                Err(_) => number.as_f64().map_or(Value::Null, Value::F64),
            }
        }
        serde_json::Value::Array(items) => Value::List(items.iter().map(from_json).collect()),
        serde_json::Value::Object(entries) => Value::Map(
            entries
                .iter()
                .map(|(key, item)| (key.clone(), from_json(item)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_text_keeps_map_order() {
        let inner = Value::Map(
            [("z".to_string(), Value::I64(1)), ("a".to_string(), Value::Null)]
                .into_iter()
                .collect(),
        );
        let map = Value::Map(
            [
                ("_type".to_string(), Value::string("MantleEvent")),
                ("_id".to_string(), Value::string("100000")),
                ("name".to_string(), Value::string("Zoë \"picks\"")),
                ("list".to_string(), Value::List(vec![inner, Value::Bool(true)])),
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(
            to_json_text(&map),
            r#"{"_type":"MantleEvent","_id":"100000","name":"Zoë \"picks\"","list":[{"z":1,"a":null},true]}"#
        );
    }

    #[test]
    fn numbers_keep_their_kind_and_non_ascii_text_survives() {
        let json: serde_json::Value =
            serde_json::from_str(r#"{"n":3,"price":12.50,"name":"Zoë 🐘","list":[1,"a",null],"ok":true}"#).unwrap();
        let Value::Map(map) = from_json(&json) else {
            panic!("a map");
        };
        assert_eq!(map.get("n"), Some(&Value::I64(3)));
        assert_eq!(map.get("price").map(to_plain_string).as_deref(), Some("12.50"));
        assert_eq!(map.get("name").and_then(Value::as_str), Some("Zoë 🐘"));
        assert!(matches!(map.get("list"), Some(Value::List(items)) if items.len() == 3));
        let back = to_json(&Value::Map(map));
        assert_eq!(back["name"], "Zoë 🐘");
        assert_eq!(back["n"], 3);
    }
}
