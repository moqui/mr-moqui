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

//! Entity JSON data files, read one array element at a time. The streaming scan finds where each element ends; serde_json
//! parses it.

use std::collections::BTreeMap;
use std::fmt;
use std::io::BufRead;

use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

use super::loader::{
    apply_default_parameters, apply_default_values, apply_key_pairs, call_data_file_service, data_service_name,
    decode_binary, entity_defined, inherit_pk_fields, shape_of, shared_pks, skip_type, store_record, Record,
};
use super::pull::Pull;
use super::{DataFileService, LoadOptions};
use crate::entity::facade::EntityFacade;
use crate::entity::FieldMap;

pub(super) fn load_json_reader(
    facade: &EntityFacade,
    location: &str,
    reader: impl BufRead,
    options: &LoadOptions,
) -> Result<u64, Error> {
    let mut input = Pull::new(reader);
    input.skip_bom()?;
    let kind = input.take_with(json_start, "json root")?;
    match kind {
        JsonStart::Object => {
            let text = input.take_text(json_value_end, "json value")?;
            let loaded = load_json_text_value(facade, location, &text, options)?;
            input.consume_match(json_ws, "json space")?;
            if !input.exhausted()? {
                return Err(Error::new(format!("{location} json has trailing data")));
            }
            Ok(loaded)
        }
        JsonStart::Array => {
            let mut total = 0_u64;
            let mut file_type = None;
            let mut first = true;
            loop {
                input.consume_match(json_ws, "json space")?;
                if input.starts_with("]")? {
                    input.consume(1);
                    break;
                }
                if input.exhausted()? {
                    return Err(Error::new(format!("{location} json array ended early")));
                }
                let text = input.take_text(json_value_end, "json value")?;
                let value = parse_json(&text)?;
                let mut marker = false;
                if first {
                    first = false;
                    if let Json::Object(pairs) = &value {
                        if let Some(kind) = json_object_string(pairs, "_dataType") {
                            file_type = Some(kind);
                            marker = true;
                        }
                    }
                }
                if !marker && !skip_type(file_type.as_deref(), &options.data_types) {
                    total = total.saturating_add(load_json_document(facade, location, value, options)?);
                }
                let end = input.take_with(json_after_value, "json separator")?;
                if end {
                    break;
                }
            }
            Ok(total)
        }
    }
}

fn load_json_text_value(
    facade: &EntityFacade,
    location: &str,
    text: &str,
    options: &LoadOptions,
) -> Result<u64, Error> {
    let root = parse_json(text)?;
    let (file_type, values) = json_documents(root)?;
    if skip_type(file_type.as_deref(), &options.data_types) {
        return Ok(0);
    }
    let mut total = 0_u64;
    for value in values {
        total = total.saturating_add(load_json_document(facade, location, value, options)?);
    }
    Ok(total)
}

fn load_json_document(facade: &EntityFacade, location: &str, value: Json, options: &LoadOptions) -> Result<u64, Error> {
    let Json::Object(pairs) = value else {
        tracing::warn!(location, "json value is not an object");
        return Ok(0);
    };
    let entity_name =
        json_object_string(&pairs, "_entity").ok_or_else(|| Error::new(format!("json {location} has no _entity")))?;
    if entity_defined(facade, &entity_name) {
        let mut record = json_record(facade, &entity_name, &pairs)?;
        apply_default_values(facade, options, &mut record);
        store_record(facade, options, record)
    } else if let Some(name) = data_service_name(&entity_name) {
        let mut service = DataFileService {
            name,
            location: location.to_string(),
            parameters: json_service_parameters(&pairs)?,
        };
        apply_default_parameters(options, &mut service);
        call_data_file_service(facade, options, service)?;
        Ok(0)
    } else {
        Err(Error::new(format!(
            "json {location} _entity {entity_name} is not an entity or a service"
        )))
    }
}

fn json_documents(root: Json) -> Result<(Option<String>, Vec<Json>), Error> {
    match root {
        Json::Object(pairs) => {
            let file_type = json_object_string(&pairs, "_dataType");
            Ok((file_type, vec![Json::Object(pairs)]))
        }
        Json::Array(mut values) => {
            let file_type = values.first().and_then(|value| {
                if let Json::Object(pairs) = value {
                    json_object_string(pairs, "_dataType")
                } else {
                    None
                }
            });
            // A leading array object that carries `_dataType` is the file type and is not a record.
            if file_type.is_some() {
                values.remove(0);
            }
            Ok((file_type, values))
        }
        _ => Err(Error::new("json root must be an object or an array")),
    }
}

fn json_record(facade: &EntityFacade, entity_name: &str, pairs: &[(String, Json)]) -> Result<Record, Error> {
    json_record_in(facade, entity_name, pairs, &BTreeMap::new())
}

fn json_record_in(
    facade: &EntityFacade,
    entity_name: &str,
    pairs: &[(String, Json)],
    ancestor: &FieldMap,
) -> Result<Record, Error> {
    let shape = shape_of(facade, entity_name)?;
    let mut fields = BTreeMap::new();
    let mut nested_values = Vec::new();
    for (key, value) in pairs {
        if key == "_entity" || key == "_dataType" {
            continue;
        }
        match value {
            Json::Object(_) | Json::Array(_) => nested_values.push((key, value)),
            _ => {
                if !shape.fields.contains_key(key.as_str()) {
                    tracing::warn!(entity = %shape.full_name, field = %key, "json field is not on the entity");
                    continue;
                }
                if let Some(converted) =
                    json_field_value(shape.fields.get(key.as_str()).map(String::as_str).unwrap_or(""), value)?
                {
                    fields.insert(shape.name(key), converted);
                }
            }
        }
    }
    let shared = shared_pks(&shape, &fields, ancestor);
    let mut children = Vec::new();
    for (key, value) in nested_values {
        let Some(rel) = shape.relations.get(key) else {
            if entity_defined(facade, key) {
                let mut nested = json_children(facade, key, value, &shared)?;
                for child in &mut nested {
                    inherit_pk_fields(facade, &child.entity, &shared, &mut child.fields)?;
                }
                children.extend(nested);
            } else {
                tracing::warn!(entity = %shape.full_name, field = %key, "json nested value is not a relationship");
            }
            continue;
        };
        let mut nested = json_children(facade, &rel.related, value, &shared)?;
        for child in &mut nested {
            apply_key_pairs(&fields, rel, &mut child.fields);
            inherit_pk_fields(facade, &child.entity, &shared, &mut child.fields)?;
        }
        children.extend(nested);
    }
    Ok(Record {
        entity: shape.full_name.clone(),
        fields,
        children,
    })
}

fn json_children(
    facade: &EntityFacade,
    entity_name: &str,
    value: &Json,
    ancestor: &FieldMap,
) -> Result<Vec<Record>, Error> {
    match value {
        Json::Object(pairs) => Ok(vec![json_record_in(facade, entity_name, pairs, ancestor)?]),
        Json::Array(values) => {
            let mut records = Vec::new();
            for value in values {
                let Json::Object(pairs) = value else {
                    return Err(Error::new(format!("nested {entity_name} value is not an object")));
                };
                records.push(json_record_in(facade, entity_name, pairs, ancestor)?);
            }
            Ok(records)
        }
        _ => Err(Error::new(format!("nested {entity_name} value is not an object"))),
    }
}

fn json_service_parameters(pairs: &[(String, Json)]) -> Result<BTreeMap<String, String>, Error> {
    let mut parameters = BTreeMap::new();
    for (key, value) in pairs {
        if key == "_entity" || key == "_dataType" {
            continue;
        }
        match value {
            Json::Object(_) | Json::Array(_) => {
                return Err(Error::new(format!("service parameter {key} is not a scalar")));
            }
            Json::Null => {}
            Json::String(text) if text.is_empty() => {}
            other => {
                if let Some(text) = json_scalar_string(other) {
                    parameters.insert(key.clone(), text);
                }
            }
        }
    }
    Ok(parameters)
}

fn json_object_string(pairs: &[(String, Json)], key: &str) -> Option<String> {
    pairs.iter().find_map(|(name, value)| {
        if name == key {
            json_scalar_string(value).filter(|text| !text.is_empty())
        } else {
            None
        }
    })
}

fn json_scalar_string(value: &Json) -> Option<String> {
    match value {
        Json::Null => None,
        Json::Bool(flag) => Some(flag.to_string()),
        Json::Number(text) => Some(text.clone()),
        Json::String(text) => Some(text.clone()),
        Json::Array(_) | Json::Object(_) => None,
    }
}

fn json_field_value(field_type: &str, value: &Json) -> Result<Option<Value>, Error> {
    match value {
        // Java puts a null or empty value as null, so a store clears the column.
        Json::Null => Ok(Some(Value::Null)),
        Json::String(text) if text.is_empty() => Ok(Some(Value::Null)),
        // Base64, as this port's data writer writes bytes in JSON, so the two round-trip; Java's JSON reader stores the
        // text's UTF-8 bytes, and its writer has no text form for bytes. Text that is not Base64 is its UTF-8 bytes.
        Json::String(text) if field_type == "binary-very-long" => Ok(Some(Value::Bytes(decode_binary(text)))),
        Json::String(text) => Ok(Some(Value::string(text))),
        Json::Bool(flag) => Ok(Some(Value::Bool(*flag))),
        // The text as written: `convert_fields` converts it by the field type, so no digits are lost to `f64`.
        Json::Number(text) => Ok(Some(Value::string(text))),
        Json::Array(_) | Json::Object(_) => Ok(None),
    }
}

enum JsonStart {
    Object,
    Array,
}

fn json_ws(text: &str, eof: bool) -> Result<Option<usize>, Error> {
    let n = text.bytes().take_while(|byte| is_json_ws(*byte)).count();
    if n == text.len() && !eof {
        return Ok(None);
    }
    Ok(Some(n))
}

fn json_start(text: &str, eof: bool) -> Result<Option<(usize, JsonStart)>, Error> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() && is_json_ws(bytes[i]) {
        i += 1;
    }
    if i >= bytes.len() {
        return if eof {
            Err(Error::new("json ended early"))
        } else {
            Ok(None)
        };
    }
    match bytes[i] {
        b'{' => Ok(Some((i, JsonStart::Object))),
        b'[' => Ok(Some((i + 1, JsonStart::Array))),
        found => Err(Error::new(format!("unexpected json character {}", char::from(found)))),
    }
}

pub(super) fn json_value_end(text: &str, eof: bool) -> Result<Option<usize>, Error> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() && is_json_ws(bytes[i]) {
        i += 1;
    }
    if i >= bytes.len() {
        return if eof {
            Err(Error::new("json ended early"))
        } else {
            Ok(None)
        };
    }
    match bytes[i] {
        b'"' => scan_string(bytes, i, eof),
        b'{' | b'[' => scan_container(bytes, i, eof),
        b't' => scan_lit(bytes, i, b"true", eof),
        b'f' => scan_lit(bytes, i, b"false", eof),
        b'n' => scan_lit(bytes, i, b"null", eof),
        b'-' | b'0'..=b'9' => scan_number(bytes, i, eof),
        found => Err(Error::new(format!("unexpected json character {}", char::from(found)))),
    }
}

fn json_after_value(text: &str, eof: bool) -> Result<Option<(usize, bool)>, Error> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() && is_json_ws(bytes[i]) {
        i += 1;
    }
    if i >= bytes.len() {
        return if eof {
            Err(Error::new("json separator ended early"))
        } else {
            Ok(None)
        };
    }
    match bytes[i] {
        b',' => Ok(Some((i + 1, false))),
        b']' => Ok(Some((i + 1, true))),
        found => Err(Error::new(format!("unexpected json character {}", char::from(found)))),
    }
}

fn scan_string(bytes: &[u8], start: usize, eof: bool) -> Result<Option<usize>, Error> {
    let mut i = start + 1;
    let mut escape = false;
    while i < bytes.len() {
        let byte = bytes[i];
        i += 1;
        if escape {
            escape = false;
            continue;
        }
        if byte == b'\\' {
            escape = true;
            continue;
        }
        if byte == b'"' {
            return Ok(Some(i));
        }
    }
    if eof {
        Err(Error::new("json string ended early"))
    } else {
        Ok(None)
    }
}

fn scan_container(bytes: &[u8], start: usize, eof: bool) -> Result<Option<usize>, Error> {
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escape = false;
    let mut i = start;
    while i < bytes.len() {
        let byte = bytes[i];
        i += 1;
        if in_string {
            if escape {
                escape = false;
                continue;
            }
            if byte == b'\\' {
                escape = true;
                continue;
            }
            if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(Some(i));
                }
                if depth < 0 {
                    return Err(Error::new("json container is not closed"));
                }
            }
            _ => {}
        }
    }
    if eof {
        Err(Error::new("json ended early"))
    } else {
        Ok(None)
    }
}

fn scan_lit(bytes: &[u8], start: usize, lit: &[u8], eof: bool) -> Result<Option<usize>, Error> {
    let have = bytes.len().saturating_sub(start);
    if have < lit.len() {
        if bytes[start..].iter().zip(lit.iter()).all(|(got, want)| got == want) {
            return if eof {
                Err(Error::new("json literal is incomplete"))
            } else {
                Ok(None)
            };
        }
        return Err(Error::new("json literal is incomplete"));
    }
    if &bytes[start..start + lit.len()] != lit {
        return Err(Error::new("json literal is incomplete"));
    }
    if start + lit.len() == bytes.len() && !eof {
        return Ok(None);
    }
    Ok(Some(start + lit.len()))
}

fn scan_number(bytes: &[u8], start: usize, eof: bool) -> Result<Option<usize>, Error> {
    let mut i = start;
    if i < bytes.len() && bytes[i] == b'-' {
        i += 1;
    }
    if i >= bytes.len() {
        return number_end(i, eof, false);
    }
    if bytes[i] == b'0' {
        i += 1;
    } else if bytes[i].is_ascii_digit() {
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    } else {
        return Err(Error::new("json number is incomplete"));
    }
    if i >= bytes.len() {
        return number_end(i, eof, true);
    }
    if bytes[i] == b'.' {
        i += 1;
        if i >= bytes.len() {
            return number_end(i, eof, false);
        }
        if !bytes[i].is_ascii_digit() {
            return Err(Error::new("json fraction is incomplete"));
        }
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i >= bytes.len() {
            return number_end(i, eof, true);
        }
    }
    if bytes[i] == b'e' || bytes[i] == b'E' {
        i += 1;
        if i >= bytes.len() {
            return number_end(i, eof, false);
        }
        if bytes[i] == b'+' || bytes[i] == b'-' {
            i += 1;
        }
        if i >= bytes.len() {
            return number_end(i, eof, false);
        }
        if !bytes[i].is_ascii_digit() {
            return Err(Error::new("json exponent is incomplete"));
        }
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i >= bytes.len() {
            return number_end(i, eof, true);
        }
    }
    Ok(Some(i))
}

fn number_end(i: usize, eof: bool, complete: bool) -> Result<Option<usize>, Error> {
    if !eof {
        Ok(None)
    } else if complete {
        Ok(Some(i))
    } else {
        Err(Error::new("json number is incomplete"))
    }
}

fn is_json_ws(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

/// One JSON value. A number keeps its text, so a field converts it by its type and a decimal stays exact. Object
/// members stay in document order, so nested records are written in the order they appear.
pub(super) enum Json {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

/// The key under which serde_json, built with `arbitrary_precision`, hands a number to a visitor: a one-member map of
/// the number's text.
const NUMBER_TOKEN: &str = "$serde_json::private::Number";

/// Parse one JSON value. serde_json stops nesting past 128 levels with an error.
pub(super) fn parse_json(text: &str) -> Result<Json, Error> {
    serde_json::from_str(text).map_err(|err| Error::new(format!("json is not valid: {err}")).with_source(err))
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JsonVisitor)
    }
}

struct JsonVisitor;

impl<'de> Visitor<'de> for JsonVisitor {
    type Value = Json;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Json, E> {
        Ok(Json::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Json, E> {
        Ok(Json::Number(value.to_string()))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Json, E> {
        Ok(Json::Number(value.to_string()))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Json, E> {
        Ok(Json::Number(value.to_string()))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Json, E> {
        Ok(Json::String(value.to_string()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Json, E> {
        Ok(Json::String(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element()? {
            values.push(value);
        }
        Ok(Json::Array(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
        let mut pairs = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            if pairs.is_empty() && key == NUMBER_TOKEN {
                return Ok(Json::Number(map.next_value()?));
            }
            pairs.push((key, map.next_value()?));
        }
        Ok(Json::Object(pairs))
    }
}
