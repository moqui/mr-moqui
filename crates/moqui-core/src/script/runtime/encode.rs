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

//! The text encodings and value shapes the screen macros' helpers (`WebUtilities`, `CollectionUtilities`) use.

use moqui_lib::object::to_plain_string;

use super::*;

/// The SVG options of a `code128Svg` options map.
pub(crate) fn code128_options(value: &Value) -> Result<moqui_lib::barcode::SvgOptions, String> {
    let Value::Map(map) = value else {
        return Err("the options are not an object".to_string());
    };
    let length = |value: &Value| match value {
        Value::String(text) => moqui_lib::barcode::length_mm(text).map_err(|err| err.message().to_string()),
        other => to_plain_string(other)
            .parse::<f64>()
            .map_err(|_| format!("{} is not a length", to_plain_string(other))),
    };
    let mut options = moqui_lib::barcode::SvgOptions::default();
    for (name, value) in map {
        match name.as_str() {
            "height" => options.height = length(value)?,
            "moduleWidth" => options.module_width = length(value)?,
            "fontSize" => options.font_size = length(value)?,
            "fontFamily" | "fontName" => options.font_family = to_plain_string(value),
            "humanReadable" => options.human_readable = !matches!(value, Value::Bool(false) | Value::Null),
            "quietZone" => {
                options.quiet_zone = match value {
                    Value::Bool(false) | Value::Null => 0,
                    Value::Bool(true) => moqui_lib::barcode::SvgOptions::default().quiet_zone,
                    other => to_plain_string(other)
                        .parse()
                        .map_err(|_| format!("quietZone {} is not a number of modules", to_plain_string(other)))?,
                }
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(options)
}

pub(crate) fn url_safe_chars(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains("..")
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(|ch| (ch as u32) < 32 || ch as u32 == 127)
}

/// `WebUtilities.encodeHtmlJsSafe`: HTML escapes plus the escapes a single-quoted JavaScript string needs. Unlike
/// Java it also escapes `\`, which would otherwise undo the quote escape, and U+2028 and U+2029, which end a
/// JavaScript string literal in older engines.
pub(crate) fn encode_html_js_safe(original: &str) -> String {
    let mut out = String::with_capacity(original.len());
    for ch in original.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '"' => out.push_str("&quot;"),
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            '\u{5}' => out.push_str("..."),
            '\u{12}' => out.push_str("&apos;"),
            '\u{13}' | '\u{14}' => out.push_str("&quot;"),
            '\u{16}' | '\u{17}' => out.push('-'),
            '\u{19}' => out.push_str("tm"),
            other => out.push(other),
        }
    }
    out
}

fn js_safe_object(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::I32(_) | Value::I64(_) | Value::F64(_) | Value::Decimal(_) => moqui_lib::object::to_plain_string(value),
        Value::List(items) => {
            let parts: Vec<String> = items.iter().map(js_safe_object).collect();
            format!("[{}]", parts.join(","))
        }
        Value::Map(entries) => js_safe_map(entries),
        other => format!("'{}'", encode_html_js_safe(&moqui_lib::object::to_plain_string(other))),
    }
}

pub(crate) fn js_safe_map(entries: &indexmap::IndexMap<String, Value>) -> String {
    let parts: Vec<String> = entries
        .iter()
        .map(|(key, value)| format!("'{}':{}", encode_html_js_safe(key), js_safe_object(value)))
        .collect();
    format!("{{{}}}", parts.join(","))
}

/// Java's `CollectionUtilities.flattenNestedMap`: the entries of a map, an entity value's fields among them, with each
/// nested map's entries, and those of the maps in a nested list, put in its place. A later key replaces an earlier one.
/// `CollectionUtilities.flattenNestedMap(map)`: the leaves of nested maps and lists in one map; null for a value that
/// is not a map.
pub(crate) fn flatten_nested_map(value: &Value) -> Value {
    fn flatten(value: &Value, out: &mut indexmap::IndexMap<String, Value>) {
        let Some(entries) = super::values::value_map(value) else {
            return;
        };
        for (key, item) in entries {
            if super::values::value_map(&item).is_some() {
                flatten(&item, out);
            } else if let Some(items) = super::values::value_items(&item) {
                for each in &items {
                    flatten(each, out);
                }
            } else {
                out.insert(key, item);
            }
        }
    }
    if super::values::value_map(value).is_none() {
        return Value::Null;
    }
    let mut out = indexmap::IndexMap::new();
    flatten(value, &mut out);
    Value::Map(out)
}
