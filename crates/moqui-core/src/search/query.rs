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

//! The search query tree, and its map form for scripts.
//!
//! ```text
//! "match_all"
//! {text:  {terms: "jo sm", fields: "all" | ["combinedName"], match: "term"|"prefix"|"contains"|"phrase"|"wildcard"}}
//! {field: {name: "itemTypeEnumId", op: "eq"|"in"|"prefix"|"wildcard"|"range"|"exists",
//!          value, values: [..], gt, gte, lt, lte, negate: false}}
//! {bool:  {all: [..], any: [..], none: [..], min_any: 1}}
//! {child: {path: "classifications", query: {..}}}
//! {near:  {terms: "water pump", fields: "all" | [..], distance: 3}}     capability proximity
//! {fuzzy: {terms: "industral"}}                                         capability fuzzy
//! {suggest: {text: "wat pu"}}                                           capability autocomplete
//! ```

use indexmap::IndexMap;
use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextMatch {
    /// Every token, as whole tokens.
    Term,
    /// Every token, the last characters open.
    Prefix,
    /// Substring anywhere in the value.
    Contains,
    /// The tokens in order.
    Phrase,
    /// `?` and `*` wildcards inside the value.
    Wildcard,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextFields {
    All,
    Only(Vec<String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldOp {
    Eq,
    In,
    Prefix,
    Wildcard,
    Range,
    Exists,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldQuery {
    pub name: String,
    pub op: FieldOp,
    pub value: Value,
    pub values: Vec<Value>,
    pub gt: Option<Value>,
    pub gte: Option<Value>,
    pub lt: Option<Value>,
    pub lte: Option<Value>,
    pub negate: bool,
}

/// The pseudo-field naming a hit's document type, such as `MantleParty`. A clause on it is
/// decided per document type when the query compiles: trusted per-type predicates and the
/// document-type facet use it.
pub const DOCUMENT_FIELD: &str = "_document";

impl FieldQuery {
    /// For a clause on [`DOCUMENT_FIELD`], whether `document_id` matches it, `negate` applied.
    /// `None` for any other field.
    pub fn document_match(&self, document_id: &str) -> Option<bool> {
        if self.name != DOCUMENT_FIELD {
            return None;
        }
        let is = |value: &Value| to_plain_string(value) == document_id;
        let positive = match self.op {
            FieldOp::Exists => true,
            FieldOp::Eq => is(&self.value),
            FieldOp::In => self.values.iter().any(is) || (self.values.is_empty() && is(&self.value)),
            FieldOp::Prefix => document_id.starts_with(&to_plain_string(&self.value)),
            FieldOp::Wildcard | FieldOp::Range => false,
        };
        Some(positive != self.negate)
    }

    fn new(name: impl Into<String>, op: FieldOp) -> Self {
        Self {
            name: name.into(),
            op,
            value: Value::Null,
            values: Vec::new(),
            gt: None,
            gte: None,
            lt: None,
            lte: None,
            negate: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Query {
    MatchAll,
    /// The scored clause.
    Text {
        terms: String,
        fields: TextFields,
        kind: TextMatch,
    },
    /// A filter. A dotted name matches when any element of that child array matches.
    Field(Box<FieldQuery>),
    /// `min_any` is `minimum_should_match`. With none given it is 1 when `all` is empty and `any`
    /// is not, and 0 otherwise, as an Elastic `bool` query has it.
    Bool {
        all: Vec<Query>,
        any: Vec<Query>,
        none: Vec<Query>,
        min_any: Option<u32>,
    },
    /// One element of the child array at `path` satisfies the whole inner query.
    Child {
        path: String,
        query: Box<Query>,
    },
    /// The terms in this order within one field value, with at most `distance` other words between
    /// the first and the last. Scored as text. Needs [`super::Capability::Proximity`].
    Near {
        terms: String,
        fields: TextFields,
        distance: u32,
    },
    /// Typo recovery on the document's `title` role: trigram word similarity on Postgres, edit
    /// distance on OpenSearch, so the two can differ at the edges. Scored by closeness. Needs
    /// [`super::Capability::Fuzzy`]; a document with no title matches nothing.
    Fuzzy {
        terms: String,
    },
    /// Autocomplete on the `title` role: the title starts with the text, or each word of the text
    /// starts a word of the title. A title start scores higher. Needs
    /// [`super::Capability::Autocomplete`]; a document with no title matches nothing.
    Suggest {
        text: String,
    },
    /// Native typo recovery on selected mapped textual fields; omitted fuzzy fields retain the title-only API.
    FuzzyIn {
        terms: String,
        fields: TextFields,
    },
    /// Relevance multiplier; matching is exactly the inner query's matching.
    Boost {
        query: Box<Query>,
        weight: f64,
    },
    /// Legacy `{lucene: text}` wrapper. The facade normalizes it through the shared query-string parser before backend dispatch.
    Lucene(String),
}

impl Query {
    /// Resolve legacy query-string wrappers before capability checks and backend dispatch.
    pub(crate) fn normalized(&self) -> Result<Self, Error> {
        Ok(match self {
            Self::Lucene(text) => super::parse::parse_query_string(text)?,
            Self::Boost { query, weight } => {
                if !weight.is_finite() || *weight <= 0.0 {
                    return Err(Error::new("boost weight must be positive and finite"));
                }
                Self::Boost {
                    query: Box::new(query.normalized()?),
                    weight: *weight,
                }
            }
            Self::Bool {
                all,
                any,
                none,
                min_any,
            } => Self::Bool {
                all: all.iter().map(Self::normalized).collect::<Result<_, _>>()?,
                any: any.iter().map(Self::normalized).collect::<Result<_, _>>()?,
                none: none.iter().map(Self::normalized).collect::<Result<_, _>>()?,
                min_any: *min_any,
            },
            Self::Child { path, query } => Self::child(path, query.normalized()?),
            other => other.clone(),
        })
    }

    /// The capabilities this tree needs, each once.
    pub fn capabilities(&self) -> Vec<super::Capability> {
        fn walk(query: &Query, out: &mut Vec<super::Capability>) {
            let need = match query {
                Query::Near { .. } => Some(super::Capability::Proximity),
                Query::Fuzzy { .. } | Query::FuzzyIn { .. } => Some(super::Capability::Fuzzy),
                Query::Suggest { .. } => Some(super::Capability::Autocomplete),
                Query::Bool { all, any, none, .. } => {
                    for item in all.iter().chain(any).chain(none) {
                        walk(item, out);
                    }
                    None
                }
                Query::Child { query, .. } | Query::Boost { query, .. } => {
                    walk(query, out);
                    None
                }
                _ => None,
            };
            if let Some(need) = need {
                if !out.contains(&need) {
                    out.push(need);
                }
            }
        }
        let mut out = Vec::new();
        walk(self, &mut out);
        out
    }

    pub fn eq(name: impl Into<String>, value: Value) -> Self {
        let mut field = FieldQuery::new(name, FieldOp::Eq);
        field.value = value;
        Self::Field(Box::new(field))
    }

    pub fn is_in(name: impl Into<String>, values: Vec<Value>) -> Self {
        let mut field = FieldQuery::new(name, FieldOp::In);
        field.values = values;
        Self::Field(Box::new(field))
    }

    pub fn prefix(name: impl Into<String>, value: impl Into<String>) -> Self {
        let mut field = FieldQuery::new(name, FieldOp::Prefix);
        field.value = Value::String(value.into());
        Self::Field(Box::new(field))
    }

    pub fn exists(name: impl Into<String>) -> Self {
        Self::Field(Box::new(FieldQuery::new(name, FieldOp::Exists)))
    }

    /// `gte` and `lt`, the sales date window.
    pub fn range(name: impl Into<String>, gte: Option<Value>, lt: Option<Value>) -> Self {
        let mut field = FieldQuery::new(name, FieldOp::Range);
        field.gte = gte;
        field.lt = lt;
        Self::Field(Box::new(field))
    }

    pub fn range_full(
        name: impl Into<String>,
        gt: Option<Value>,
        gte: Option<Value>,
        lt: Option<Value>,
        lte: Option<Value>,
    ) -> Self {
        let mut field = FieldQuery::new(name, FieldOp::Range);
        field.gt = gt;
        field.gte = gte;
        field.lt = lt;
        field.lte = lte;
        Self::Field(Box::new(field))
    }

    pub fn all(queries: Vec<Query>) -> Self {
        Self::Bool {
            all: queries,
            any: Vec::new(),
            none: Vec::new(),
            min_any: None,
        }
    }

    pub fn any(queries: Vec<Query>) -> Self {
        Self::Bool {
            all: Vec::new(),
            any: queries,
            none: Vec::new(),
            min_any: None,
        }
    }

    /// Every document the query does not match.
    pub fn exclude(query: Query) -> Self {
        Self::Bool {
            all: Vec::new(),
            any: Vec::new(),
            none: vec![query],
            min_any: None,
        }
    }

    pub fn text(terms: impl Into<String>, kind: TextMatch) -> Self {
        Self::Text {
            terms: terms.into(),
            fields: TextFields::All,
            kind,
        }
    }

    pub fn text_in(field: impl Into<String>, terms: impl Into<String>, kind: TextMatch) -> Self {
        Self::Text {
            terms: terms.into(),
            fields: TextFields::Only(vec![field.into()]),
            kind,
        }
    }

    pub fn child(path: impl Into<String>, query: Query) -> Self {
        Self::Child {
            path: path.into(),
            query: Box::new(query),
        }
    }

    /// The negation of a field clause, the `negate` flag.
    pub fn negated(self) -> Self {
        match self {
            Self::Field(mut field) => {
                field.negate = !field.negate;
                Self::Field(field)
            }
            other => Self::exclude(other),
        }
    }

    /// Read the map form. A string is `match_all` or a query string to parse.
    ///
    /// # Errors
    ///
    /// Returns an error naming the part that is not a query.
    pub fn from_value(value: &Value) -> Result<Self, Error> {
        match value {
            Value::Null => Ok(Self::MatchAll),
            Value::String(text) if text == "match_all" => Ok(Self::MatchAll),
            Value::String(text) => super::parse::parse_query_string(text),
            Value::Map(map) => {
                if map.len() != 1 {
                    return Err(Error::new(format!(
                        "a query map has one key (text, field, bool, child, match_all), not {}",
                        map.keys().cloned().collect::<Vec<_>>().join(", ")
                    )));
                }
                let (key, body) = map.iter().next().ok_or_else(|| Error::new("empty query map"))?;
                let body_map = || -> Result<&IndexMap<String, Value>, Error> {
                    match body {
                        Value::Map(inner) => Ok(inner),
                        _ => Err(Error::new(format!("query {key} needs a map"))),
                    }
                };
                match key.as_str() {
                    "match_all" => Ok(Self::MatchAll),
                    "lucene" => match body {
                        Value::String(text) => Ok(Self::Lucene(text.clone())),
                        _ => Err(Error::new("query lucene needs a string")),
                    },
                    "text" => {
                        let inner = body_map()?;
                        let terms = string_at(inner, "terms").unwrap_or_default();
                        let fields = match inner.get("fields") {
                            None | Some(Value::Null) => TextFields::All,
                            Some(Value::String(text)) if text == "all" || text == "*" => TextFields::All,
                            Some(Value::String(text)) => TextFields::Only(vec![text.clone()]),
                            Some(Value::List(items)) => TextFields::Only(items.iter().map(to_plain_string).collect()),
                            Some(_) => return Err(Error::new("text fields must be \"all\" or a list of names")),
                        };
                        let kind = match string_at(inner, "match").as_deref() {
                            None | Some("term") => TextMatch::Term,
                            Some("prefix") => TextMatch::Prefix,
                            Some("contains") => TextMatch::Contains,
                            Some("phrase") => TextMatch::Phrase,
                            Some("wildcard") => TextMatch::Wildcard,
                            Some(other) => return Err(Error::new(format!("text match {other} is not known"))),
                        };
                        Ok(Self::Text { terms, fields, kind })
                    }
                    "field" => {
                        let inner = body_map()?;
                        let name = string_at(inner, "name").ok_or_else(|| Error::new("field query needs a name"))?;
                        let op = match string_at(inner, "op").as_deref() {
                            None | Some("eq") | Some("equals") => FieldOp::Eq,
                            Some("in") => FieldOp::In,
                            Some("prefix") => FieldOp::Prefix,
                            Some("wildcard") => FieldOp::Wildcard,
                            Some("range") => FieldOp::Range,
                            Some("exists") => FieldOp::Exists,
                            Some(other) => return Err(Error::new(format!("field op {other} is not known"))),
                        };
                        let mut field = FieldQuery::new(name, op);
                        field.value = inner.get("value").cloned().unwrap_or(Value::Null);
                        field.values = match inner.get("values") {
                            Some(Value::List(items)) => items.clone(),
                            Some(Value::Null) | None => Vec::new(),
                            Some(other) => vec![other.clone()],
                        };
                        field.gt = present(inner.get("gt"));
                        field.gte = present(inner.get("gte"));
                        field.lt = present(inner.get("lt"));
                        field.lte = present(inner.get("lte"));
                        field.negate = matches!(inner.get("negate"), Some(Value::Bool(true)));
                        Ok(Self::Field(Box::new(field)))
                    }
                    "bool" => {
                        let inner = body_map()?;
                        let min_any = match inner.get("min_any") {
                            None | Some(Value::Null) => None,
                            Some(value) => Some(
                                to_plain_string(value)
                                    .parse::<u32>()
                                    .map_err(|_| Error::new("bool min_any must be a whole number"))?,
                            ),
                        };
                        Ok(Self::Bool {
                            all: list_at(inner, "all")?,
                            any: list_at(inner, "any")?,
                            none: list_at(inner, "none")?,
                            min_any,
                        })
                    }
                    "near" => {
                        let inner = body_map()?;
                        let terms = match inner.get("terms") {
                            Some(Value::List(items)) => items.iter().map(to_plain_string).collect::<Vec<_>>().join(" "),
                            Some(value) => to_plain_string(value),
                            None => String::new(),
                        };
                        let fields = match inner.get("fields") {
                            None | Some(Value::Null) => TextFields::All,
                            Some(Value::String(text)) if text == "all" || text == "*" => TextFields::All,
                            Some(Value::String(text)) => TextFields::Only(vec![text.clone()]),
                            Some(Value::List(items)) => TextFields::Only(items.iter().map(to_plain_string).collect()),
                            Some(_) => return Err(Error::new("near fields must be \"all\" or a list of names")),
                        };
                        let distance = match inner.get("distance") {
                            None | Some(Value::Null) => 0,
                            Some(value) => to_plain_string(value)
                                .parse::<u32>()
                                .map_err(|_| Error::new("near distance must be a whole number"))?,
                        };
                        Ok(Self::Near {
                            terms,
                            fields,
                            distance,
                        })
                    }
                    "fuzzy" => {
                        let inner = body_map()?;
                        let terms = string_at(inner, "terms").unwrap_or_default();
                        let fields = match inner.get("fields") {
                            None | Some(Value::Null) => return Ok(Self::Fuzzy { terms }),
                            Some(Value::String(text)) if text == "all" => TextFields::All,
                            Some(Value::String(text)) => TextFields::Only(vec![text.clone()]),
                            Some(Value::List(items)) => TextFields::Only(items.iter().map(to_plain_string).collect()),
                            _ => return Err(Error::new("fuzzy fields must be all or names")),
                        };
                        if matches!(&fields, TextFields::Only(names) if names.iter().any(|name| name.starts_with("attributes.")))
                        {
                            return Err(Error::new("attributes support keyword filters, not fuzzy matching"));
                        }
                        Ok(Self::FuzzyIn { terms, fields })
                    }
                    "boost" => {
                        let inner = body_map()?;
                        let weight = inner
                            .get("weight")
                            .map(to_plain_string)
                            .and_then(|v| v.parse::<f64>().ok())
                            .filter(|v| v.is_finite() && *v > 0.0)
                            .ok_or_else(|| Error::new("boost weight must be positive and finite"))?;
                        Ok(Self::Boost {
                            query: Box::new(Self::from_value(inner.get("query").unwrap_or(&Value::Null))?),
                            weight,
                        })
                    }
                    "suggest" => Ok(Self::Suggest {
                        text: string_at(body_map()?, "text").unwrap_or_default(),
                    }),
                    "child" => {
                        let inner = body_map()?;
                        let path = string_at(inner, "path").ok_or_else(|| Error::new("child query needs a path"))?;
                        let query = Self::from_value(inner.get("query").unwrap_or(&Value::Null))?;
                        Ok(Self::child(path, query))
                    }
                    other => Err(Error::new(format!("query type {other} is not known"))),
                }
            }
            _ => Err(Error::new("a query is a map or a string")),
        }
    }

    /// The map form [`Self::from_value`] reads.
    pub fn to_value(&self) -> Value {
        let wrap = |key: &str, inner: IndexMap<String, Value>| {
            let mut map = IndexMap::new();
            map.insert(key.to_string(), Value::Map(inner));
            Value::Map(map)
        };
        match self {
            Self::MatchAll => Value::string("match_all"),
            Self::Lucene(text) => {
                let mut map = IndexMap::new();
                map.insert("lucene".to_string(), Value::string(text));
                Value::Map(map)
            }
            Self::Near {
                terms,
                fields,
                distance,
            } => {
                let mut inner = IndexMap::new();
                inner.insert("terms".into(), Value::string(terms));
                inner.insert(
                    "fields".into(),
                    match fields {
                        TextFields::All => Value::string("all"),
                        TextFields::Only(names) => Value::List(names.iter().map(Value::string).collect()),
                    },
                );
                inner.insert("distance".into(), Value::I64(i64::from(*distance)));
                wrap("near", inner)
            }
            Self::Fuzzy { terms } => {
                let mut inner = IndexMap::new();
                inner.insert("terms".into(), Value::string(terms));
                wrap("fuzzy", inner)
            }
            Self::FuzzyIn { terms, fields } => {
                let mut inner = IndexMap::new();
                inner.insert("terms".into(), Value::string(terms));
                inner.insert(
                    "fields".into(),
                    match fields {
                        TextFields::All => Value::string("all"),
                        TextFields::Only(names) => Value::List(names.iter().map(Value::string).collect()),
                    },
                );
                wrap("fuzzy", inner)
            }
            Self::Boost { query, weight } => wrap(
                "boost",
                IndexMap::from_iter([
                    ("query".into(), query.to_value()),
                    ("weight".into(), Value::F64(*weight)),
                ]),
            ),
            Self::Suggest { text } => {
                let mut inner = IndexMap::new();
                inner.insert("text".into(), Value::string(text));
                wrap("suggest", inner)
            }
            Self::Text { terms, fields, kind } => {
                let mut inner = IndexMap::new();
                inner.insert("terms".into(), Value::string(terms));
                inner.insert(
                    "fields".into(),
                    match fields {
                        TextFields::All => Value::string("all"),
                        TextFields::Only(names) => Value::List(names.iter().map(Value::string).collect()),
                    },
                );
                let kind = match kind {
                    TextMatch::Term => "term",
                    TextMatch::Prefix => "prefix",
                    TextMatch::Contains => "contains",
                    TextMatch::Phrase => "phrase",
                    TextMatch::Wildcard => "wildcard",
                };
                inner.insert("match".into(), Value::string(kind));
                wrap("text", inner)
            }
            Self::Field(field) => {
                let mut inner = IndexMap::new();
                inner.insert("name".into(), Value::string(&field.name));
                let op = match field.op {
                    FieldOp::Eq => "eq",
                    FieldOp::In => "in",
                    FieldOp::Prefix => "prefix",
                    FieldOp::Wildcard => "wildcard",
                    FieldOp::Range => "range",
                    FieldOp::Exists => "exists",
                };
                inner.insert("op".into(), Value::string(op));
                if !field.value.is_null() {
                    inner.insert("value".into(), field.value.clone());
                }
                if !field.values.is_empty() {
                    inner.insert("values".into(), Value::List(field.values.clone()));
                }
                for (key, bound) in [
                    ("gt", &field.gt),
                    ("gte", &field.gte),
                    ("lt", &field.lt),
                    ("lte", &field.lte),
                ] {
                    if let Some(bound) = bound {
                        inner.insert(key.into(), bound.clone());
                    }
                }
                if field.negate {
                    inner.insert("negate".into(), Value::Bool(true));
                }
                wrap("field", inner)
            }
            Self::Bool {
                all,
                any,
                none,
                min_any,
            } => {
                let mut inner = IndexMap::new();
                for (key, list) in [("all", all), ("any", any), ("none", none)] {
                    if !list.is_empty() {
                        inner.insert(key.into(), Value::List(list.iter().map(Query::to_value).collect()));
                    }
                }
                if let Some(min) = min_any {
                    inner.insert("min_any".into(), Value::I64(i64::from(*min)));
                }
                wrap("bool", inner)
            }
            Self::Child { path, query } => {
                let mut inner = IndexMap::new();
                inner.insert("path".into(), Value::string(path));
                inner.insert("query".into(), query.to_value());
                wrap("child", inner)
            }
        }
    }
}

/// The effective `minimum_should_match` of a bool.
pub fn effective_min_any(all: &[Query], any: &[Query], min_any: Option<u32>) -> u32 {
    min_any.unwrap_or(u32::from(all.is_empty() && !any.is_empty()))
}

fn present(value: Option<&Value>) -> Option<Value> {
    match value {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.clone()),
    }
}

fn string_at(map: &IndexMap<String, Value>, key: &str) -> Option<String> {
    match map.get(key) {
        None | Some(Value::Null) => None,
        Some(value) => Some(to_plain_string(value)),
    }
}

fn list_at(map: &IndexMap<String, Value>, key: &str) -> Result<Vec<Query>, Error> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::List(items)) => items.iter().map(Query::from_value).collect(),
        Some(other) => Ok(vec![Query::from_value(other)?]),
    }
}

/// Explicit/all-field fuzzy matching excludes the internal keyword attribute representation.
pub(crate) fn fuzzy_field_names(mapping: &super::DocumentMapping, fields: &TextFields) -> Vec<String> {
    mapping
        .textual_fields()
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| {
            !name.starts_with("__attributes.")
                && match fields {
                    TextFields::All => true,
                    TextFields::Only(names) => names.contains(name),
                }
        })
        .collect()
}

pub(crate) fn has_score_extensions(query: &Query) -> bool {
    match query {
        Query::Boost { .. } | Query::FuzzyIn { .. } => true,
        Query::Bool { all, any, none, .. } => all.iter().chain(any).chain(none).any(has_score_extensions),
        Query::Child { query, .. } => has_score_extensions(query),
        Query::Lucene(text) => super::parse::parse_query_string(text).is_ok_and(|query| has_score_extensions(&query)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_form_round_trips() {
        let query = Query::Bool {
            all: vec![
                Query::range("placedDate", Some(Value::I64(1)), Some(Value::I64(2))),
                Query::is_in("statusId", vec![Value::string("OrderPlaced")]),
            ],
            any: vec![Query::text("jo", TextMatch::Prefix)],
            none: vec![Query::exists("thruDate")],
            min_any: Some(1),
        };
        assert_eq!(Query::from_value(&query.to_value()).unwrap(), query);
        let child = Query::child(
            "classifications",
            Query::eq("partyClassificationId", Value::string("X")),
        );
        assert_eq!(Query::from_value(&child.to_value()).unwrap(), child);
        let lucene = Query::Lucene("level:ERROR AND message:timeout".into());
        assert_eq!(Query::from_value(&lucene.to_value()).unwrap(), lucene);
    }

    #[test]
    fn min_any_defaults_like_elastic() {
        let should = vec![Query::MatchAll];
        assert_eq!(effective_min_any(&[], &should, None), 1);
        assert_eq!(effective_min_any(&should, &should, None), 0);
        assert_eq!(effective_min_any(&[], &[], None), 0);
        assert_eq!(effective_min_any(&should, &should, Some(1)), 1);
    }

    #[test]
    fn extension_map_forms_round_trip_and_name_their_capabilities() {
        let json = |text: &str| moqui_lib::json::from_json(&serde_json::from_str(text).unwrap());
        let near = Query::from_value(&json(r#"{"near": {"terms": ["water", "pump"], "distance": 3}}"#)).unwrap();
        assert_eq!(
            near,
            Query::Near {
                terms: "water pump".into(),
                fields: TextFields::All,
                distance: 3
            }
        );
        let tree = Query::all(vec![
            near.clone(),
            Query::from_value(&json(r#"{"fuzzy": {"terms": "industral"}}"#)).unwrap(),
            Query::exclude(Query::from_value(&json(r#"{"suggest": {"text": "wat"}}"#)).unwrap()),
            Query::from_value(&json(r#"{"text": {"terms": "x"}}"#)).unwrap(),
        ]);
        assert_eq!(Query::from_value(&tree.to_value()).unwrap(), tree);
        assert_eq!(
            tree.capabilities(),
            vec![
                crate::search::Capability::Proximity,
                crate::search::Capability::Fuzzy,
                crate::search::Capability::Autocomplete
            ]
        );
        assert!(Query::MatchAll.capabilities().is_empty());
    }
}
