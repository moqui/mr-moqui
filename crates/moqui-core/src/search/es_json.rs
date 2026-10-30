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

//! The Elastic query JSON subset that unmigrated callers still pass in `nestedQueryMap`, read
//! into a [`Query`]. Only `search#DataDocuments` uses this.
//!
//! `bool` (`must`, `filter`, `should`, `must_not`, `minimum_should_match`), `term`, `terms`,
//! `match`, `range` (`gt`, `gte`, `lt`, `lte`), `exists`, `prefix`, `wildcard`, `match_all`,
//! `nested`, and `query_string`. Anything else is an error that names it.

use indexmap::IndexMap;
use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

use super::query::{FieldOp, FieldQuery, Query, TextFields, TextMatch};

/// Read an Elastic query given as a map or as JSON text.
///
/// # Errors
///
/// Returns an error for JSON that does not parse or a query type outside the subset.
pub fn from_es(value: &Value) -> Result<Query, Error> {
    match value {
        Value::String(text) => {
            let json: serde_json::Value =
                serde_json::from_str(text).map_err(|err| Error::new(format!("nested query is not JSON: {err}")))?;
            from_es(&moqui_lib::json::from_json(&json))
        }
        Value::Map(map) => from_map(map),
        Value::Null => Ok(Query::MatchAll),
        _ => Err(Error::new("an Elastic query is a map or JSON text")),
    }
}

/// `nestedQueryMap`: a path key makes a [`Query::Child`]; a null or empty key is a top-level
/// clause. All entries must match.
///
/// # Errors
///
/// Returns an error when an entry is not in the supported subset.
pub fn from_nested_map(map: &IndexMap<String, Value>) -> Result<Vec<Query>, Error> {
    let mut out = Vec::with_capacity(map.len());
    for (path, value) in map {
        let inner = from_es(value)?;
        if path.is_empty() || path == "null" {
            out.push(inner);
        } else {
            out.push(Query::child(path.clone(), inner));
        }
    }
    Ok(out)
}

pub(super) fn from_map(map: &IndexMap<String, Value>) -> Result<Query, Error> {
    if map.len() != 1 {
        return Err(Error::new(format!(
            "an Elastic query has one key, not {}",
            map.keys().cloned().collect::<Vec<_>>().join(", ")
        )));
    }
    let (kind, body) = map.iter().next().ok_or_else(|| Error::new("empty Elastic query"))?;
    match kind.as_str() {
        "match_all" => Ok(Query::MatchAll),
        "bool" => {
            let body = as_map(kind, body)?;
            let mut all = list(body.get("must"))?;
            all.extend(list(body.get("filter"))?);
            let any = list(body.get("should"))?;
            let none = list(body.get("must_not"))?;
            let min_any = match body.get("minimum_should_match") {
                None | Some(Value::Null) => None,
                Some(value) => Some(
                    to_plain_string(value)
                        .parse::<u32>()
                        .map_err(|_| Error::new("minimum_should_match must be a whole number"))?,
                ),
            };
            Ok(Query::Bool {
                all,
                any,
                none,
                min_any,
            })
        }
        "term" => {
            let (field, value) = single_field(kind, body)?;
            let value = match &value {
                Value::Map(inner) => inner.get("value").cloned().unwrap_or(Value::Null),
                other => other.clone(),
            };
            Ok(Query::eq(field, value))
        }
        "terms" => {
            let (field, value) = single_field(kind, body)?;
            let values = match value {
                Value::List(items) => items,
                other => vec![other],
            };
            Ok(Query::is_in(field, values))
        }
        "match" | "match_phrase" => {
            let (field, value) = single_field(kind, body)?;
            let value = match &value {
                Value::Map(inner) => inner.get("query").cloned().unwrap_or(Value::Null),
                other => other.clone(),
            };
            Ok(Query::Text {
                terms: to_plain_string(&value),
                fields: TextFields::Only(vec![field]),
                kind: if kind == "match" {
                    TextMatch::Term
                } else {
                    TextMatch::Phrase
                },
            })
        }
        "range" => {
            let (field, value) = single_field(kind, body)?;
            let bounds = as_map("range", &value)?;
            let bound = |key: &str| match bounds.get(key) {
                None | Some(Value::Null) => None,
                Some(value) => Some(value.clone()),
            };
            Ok(Query::range_full(
                field,
                bound("gt"),
                bound("gte"),
                bound("lt"),
                bound("lte"),
            ))
        }
        "exists" => {
            let body = as_map(kind, body)?;
            let field = body
                .get("field")
                .map(to_plain_string)
                .ok_or_else(|| Error::new("exists needs a field"))?;
            Ok(Query::exists(field))
        }
        "prefix" | "wildcard" => {
            let (field, value) = single_field(kind, body)?;
            let value = match &value {
                Value::Map(inner) => inner.get("value").cloned().unwrap_or(Value::Null),
                other => other.clone(),
            };
            let mut query = FieldQuery {
                name: field,
                op: if kind == "prefix" {
                    FieldOp::Prefix
                } else {
                    FieldOp::Wildcard
                },
                value,
                values: Vec::new(),
                gt: None,
                gte: None,
                lt: None,
                lte: None,
                negate: false,
            };
            query.value = Value::string(to_plain_string(&query.value));
            Ok(Query::Field(Box::new(query)))
        }
        "nested" => {
            let body = as_map(kind, body)?;
            let path = body
                .get("path")
                .map(to_plain_string)
                .ok_or_else(|| Error::new("nested needs a path"))?;
            let inner = from_es(body.get("query").unwrap_or(&Value::Null))?;
            Ok(Query::child(path, inner))
        }
        "query_string" => {
            let body = as_map(kind, body)?;
            let text = body.get("query").map(to_plain_string).unwrap_or_default();
            super::parse::parse_query_string(&text)
        }
        other => Err(Error::new(format!("Elastic query type {other} is not supported"))),
    }
}

fn as_map<'a>(kind: &str, value: &'a Value) -> Result<&'a IndexMap<String, Value>, Error> {
    match value {
        Value::Map(map) => Ok(map),
        _ => Err(Error::new(format!("Elastic {kind} needs an object"))),
    }
}

fn single_field(kind: &str, value: &Value) -> Result<(String, Value), Error> {
    let map = as_map(kind, value)?;
    let mut entries = map.iter().filter(|(key, _)| !matches!(key.as_str(), "boost" | "_name"));
    match (entries.next(), entries.next()) {
        (Some((field, value)), None) => Ok((field.clone(), value.clone())),
        _ => Err(Error::new(format!("Elastic {kind} names one field"))),
    }
}

fn list(value: Option<&Value>) -> Result<Vec<Query>, Error> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::List(items)) => items.iter().map(from_es).collect(),
        Some(other) => Ok(vec![from_es(other)?]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(text: &str) -> Value {
        Value::string(text)
    }

    #[test]
    fn party_classification_map() {
        // PartyServices.search#Party, as a Groovy map, here as the same JSON.
        let query = from_es(&json(
            r#"{"bool":{"filter":[
                {"term":{"classifications.partyClassificationId":"CustVip"}},
                {"range":{"classifications.classFromDate":{"lte":1767225600000}}}
            ],"should":[
                {"range":{"classifications.classThruDate":{"gte":1767225600000}}},
                {"bool":{"must_not":[{"exists":{"field":"classifications.classThruDate"}}]}}
            ],"minimum_should_match":1}}"#,
        ))
        .unwrap();
        let Query::Bool {
            all,
            any,
            none,
            min_any,
        } = query
        else {
            panic!("bool");
        };
        assert_eq!(all.len(), 2);
        assert_eq!(any.len(), 2);
        assert!(none.is_empty());
        assert_eq!(min_any, Some(1));
        assert_eq!(
            all[0],
            Query::eq("classifications.partyClassificationId", Value::string("CustVip"))
        );
    }

    #[test]
    fn pop_store_category_text() {
        // PopRestStore search#ProductSearchResults: must with match, range, and a should-only bool.
        let mut map = IndexMap::new();
        map.insert(
            "categories".to_string(),
            json(
                r#"{"bool":{"must":[ {"match":{"categories.productCategoryId":"PopcAllProducts"}},
                    {"range":{"categories.categoryFromDate":{"lte":1767225600000}}},
                    {"bool":{"should":[{"range":{"categories.categoryThruDate":{"gte":1767225600000}}},
                        {"bool":{"must_not":{"exists":{"field":"categories.categoryThruDate"}}}}]}}
                ]}}"#,
            ),
        );
        let queries = from_nested_map(&map).unwrap();
        let Query::Child { path, query } = &queries[0] else {
            panic!("child");
        };
        assert_eq!(path, "categories");
        let Query::Bool { all, .. } = query.as_ref() else {
            panic!("bool");
        };
        assert_eq!(all.len(), 3);
        assert!(matches!(&all[2], Query::Bool { any, min_any: None, .. } if any.len() == 2));
    }

    #[test]
    fn unknown_types_are_named() {
        let err = from_es(&json(r#"{"fuzzy":{"name":"x"}}"#)).unwrap_err();
        assert!(err.to_string().contains("fuzzy"), "{err}");
    }
}
