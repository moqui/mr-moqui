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
//! Groovy's `JsonSlurper` reads them, so a price does not pick up binary rounding. [`canonical`] is RFC 8785 JSON, for
//! hashes over JSON values.

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

/// RFC 8785 (JCS) canonical JSON: object members sorted by the UTF-16 code units of their names, no white space,
/// strings with only the escapes JSON requires, numbers as ECMAScript writes them. Two values that are equal as JSON
/// give the same text, whatever order their maps kept, so a hash over it is stable (the approval and held-write hashes,
/// plans/LLM-A2A-Assist.md 4.7 and 4.8).
pub fn canonical(value: &serde_json::Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

/// SHA-256 of [`canonical`], as lowercase hex.
pub fn canonical_sha256(value: &serde_json::Value) -> String {
    use sha2::Digest as _;
    crate::codec::hex_encode(&sha2::Sha256::digest(canonical(value).as_bytes()))
}

fn write_canonical(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        serde_json::Value::Number(number) => out.push_str(&canonical_number(number)),
        serde_json::Value::String(text) => write_string(text, out),
        serde_json::Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        serde_json::Value::Object(entries) => {
            let mut members: Vec<(&String, &serde_json::Value)> = entries.iter().collect();
            members.sort_by(|(left, _), (right, _)| left.encode_utf16().cmp(right.encode_utf16()));
            out.push('{');
            for (index, (name, item)) in members.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(name, out);
                out.push(':');
                write_canonical(item, out);
            }
            out.push('}');
        }
    }
}

fn write_string(text: &str, out: &mut String) {
    use std::fmt::Write as _;
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if u32::from(ch) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(ch));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

/// A number as ECMAScript's `Number.prototype.toString` writes it: whole numbers below 1e21 without an exponent, the
/// shortest digits that read back the same, an exponent (`1e-7`, `1e+21`) outside 1e-6 to 1e21.
fn canonical_number(number: &serde_json::Number) -> String {
    if let Some(whole) = number.as_i64() {
        return whole.to_string();
    }
    if let Some(whole) = number.as_u64() {
        return whole.to_string();
    }
    let Some(float) = number.as_f64().filter(|float| float.is_finite()) else {
        return number.to_string();
    };
    if float == 0.0 {
        return "0".to_string();
    }
    let size = float.abs();
    if (1e-6..1e21).contains(&size) {
        if float.fract() == 0.0 {
            return format!("{float:.0}");
        }
        return format!("{float}");
    }
    let shown = format!("{float:e}");
    match shown.split_once('e') {
        Some((digits, exponent)) if !exponent.starts_with('-') => format!("{digits}e+{exponent}"),
        _ => shown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_sorts_members_and_writes_numbers_and_strings_as_rfc_8785() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"b":[1,2.5,1e21,1e-7,-0.0,10.0],"a":{"z":null,"\u20ac":"\u00e9\n\"\\\u0001/","\ud83d\ude00":true,"\ufb33":1}}"#,
        )
        .unwrap();
        assert_eq!(
            canonical(&json),
            "{\"a\":{\"z\":null,\"\u{20ac}\":\"\u{e9}\\n\\\"\\\\\\u0001/\",\"\u{1f600}\":true,\"\u{fb33}\":1},\"b\":[1,2.5,1e+21,1e-7,0,10]}"
        );
        let same: serde_json::Value = serde_json::from_str(r#"{"a":{"\ufb33":1,"\ud83d\ude00":true,"\u20ac":"\u00e9\n\"\\\u0001/","z":null},"b":[1,2.5,1e21,1e-7,0,10]}"#).unwrap();
        assert_eq!(canonical_sha256(&json), canonical_sha256(&same));
        assert_eq!(canonical_sha256(&json).len(), 64);
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
