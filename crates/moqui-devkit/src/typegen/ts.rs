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

//! TypeScript text: quoted keys, identifiers, and the type of a Java type name.

/// A string literal, for a property key or a literal type.
pub(super) fn quote(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
}

/// An identifier made from `text`: every character that is not a letter, a digit or `_` becomes `_`.
pub(super) fn ident(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() || out.starts_with(|ch: char| ch.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

/// A context name the generator can prove: a plain identifier, not a path such as `a.b` or an expansion.
pub(super) fn is_name(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
        && !text.starts_with(|ch: char| ch.is_ascii_digit())
}

/// Text that is safe inside a `/** */` comment.
pub(super) fn doc(text: &str) -> String {
    text.replace("*/", "* /").replace('\n', " ")
}

fn short(java_type: &str) -> &str {
    java_type.rsplit('.').next().unwrap_or(java_type)
}

/// The type a script reads on the server for a field or parameter of `java_type`: a decimal is the host value, a
/// timestamp is text, and binary cannot be read by a script.
pub(super) fn server_type(java_type: &str) -> &'static str {
    match short(java_type) {
        "" => "unknown",
        "String" | "Character" | "CharSequence" | "GString" => "string",
        "BigDecimal" => "Decimal",
        "Timestamp" => "Timestamp",
        "Date" | "Time" => "string",
        "Long" | "Integer" | "Short" | "Byte" | "Double" | "Float" | "BigInteger" | "Number" => "number",
        "Boolean" => "boolean",
        "Map" | "HashMap" | "LinkedHashMap" | "TreeMap" => "MoquiMap",
        "List" | "ArrayList" | "Collection" | "Set" | "HashSet" | "TreeSet" | "LinkedList" => "unknown[]",
        "EntityValue" => "EntityValue",
        "EntityList" => "EntityList",
        "Blob" | "SerialBlob" | "byte[]" => "never",
        _ => "unknown",
    }
}

/// The type a service input accepts for a parameter of `java_type`: what [`server_type`] reads, or text and numbers
/// that the validation converts to it.
pub(super) fn input_type(java_type: &str) -> &'static str {
    match server_type(java_type) {
        "Decimal" => "DecimalInput",
        "Timestamp" => "TimestampInput",
        "number" => "number | string",
        "boolean" => "boolean | string",
        "never" => "unknown",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn java_types_map_to_the_server_and_input_types() {
        assert_eq!(super::server_type("java.math.BigDecimal"), "Decimal");
        assert_eq!(super::input_type("java.math.BigDecimal"), "DecimalInput");
        assert_eq!(super::server_type("java.sql.Timestamp"), "Timestamp");
        assert_eq!(super::server_type("java.sql.Date"), "string");
        assert_eq!(super::server_type("Long"), "number");
        assert_eq!(super::input_type("Integer"), "number | string");
        assert_eq!(super::server_type("java.sql.Blob"), "never");
        assert_eq!(super::server_type("org.moqui.resource.ResourceReference"), "unknown");
        assert_eq!(super::server_type(""), "unknown");
    }

    #[test]
    fn names_and_identifiers() {
        assert!(super::is_name("orderList") && super::is_name("$x") && !super::is_name("a.b"));
        assert!(!super::is_name("${x}") && !super::is_name("1a") && !super::is_name(""));
        assert_eq!(super::ident("moqui.basic.Enumeration"), "moqui_basic_Enumeration");
        assert_eq!(super::ident("1x"), "_1x");
        assert_eq!(super::quote("a\"b"), "\"a\\\"b\"");
        assert_eq!(super::doc("a */ b"), "a * / b");
    }
}
