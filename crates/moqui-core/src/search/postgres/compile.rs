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

//! [`Query`] to a Postgres `WHERE` expression with `?` parameters.
//!
//! Root fields are typed columns. Child arrays stay in `body`: a dotted field is an `EXISTS` over
//! `jsonb_array_elements`, true when any element matches, and a [`Query::Child`] is one `EXISTS`
//! for the whole inner query, so one element satisfies every inner clause.
//!
//! Matching follows the field kind. Text is tokenized with the `simple` configuration
//! (lower-cased, no stemming, no stop words), close to Elastic's `standard` analyzer. Keyword is
//! exact and case-sensitive. A bare term searches the generated all-fields vector. Every leaf is
//! wrapped in `COALESCE(.., false)`, so a negation also matches a document that lacks the field,
//! as Elastic's `must_not` does.
//!
//! No `?` operator is written, because `?` is the parameter marker.

use std::cell::Cell;

use jiff::tz::TimeZone;
use jiff::Timestamp;
use moqui_lib::error::Error;
use moqui_lib::object::{basic_convert, parse_timestamp, to_plain_string, Value};

use crate::search::mapping::{DocumentMapping, FieldKind, FieldMapping, ObjectMapping};
use crate::search::query::{effective_min_any, FieldOp, FieldQuery, Query, TextFields, TextMatch};

/// The default text search configuration. `simple` is the closest to Elastic's `standard` analyzer.
pub const TS_CONFIG: &str = "simple";

/// The text search configuration a store uses, and the function that folds a title for fuzzy and
/// autocomplete matching.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextSetup {
    /// A `regconfig` name: `simple`, or the store's accent-folding copy.
    pub config: String,
    /// A SQL function of one text argument: `lower`, or the store's lower-and-unaccent wrapper.
    pub fold: String,
}

impl Default for TextSetup {
    fn default() -> Self {
        Self {
            config: TS_CONFIG.to_string(),
            fold: "lower".to_string(),
        }
    }
}

/// The derived title column: the `title` role folded, for fuzzy and autocomplete.
pub const NORM_TITLE: &str = "norm_title";
/// The derived weighted vector for cover-density ranking.
pub const RANKED_TEXT: &str = "ranked_text";

/// SQL text and its parameters, in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sql {
    pub text: String,
    pub params: Vec<Value>,
}

impl Sql {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            params: Vec::new(),
        }
    }

    pub fn push(&mut self, text: &str) -> &mut Self {
        self.text.push_str(text);
        self
    }

    pub fn param(&mut self, value: Value) -> &mut Self {
        self.text.push('?');
        self.params.push(value);
        self
    }

    pub fn append(&mut self, other: Sql) -> &mut Self {
        self.text.push_str(&other.text);
        self.params.extend(other.params);
        self
    }
}

/// `"name"`, doubling any quote.
pub fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `'text'`, doubling any quote.
pub fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The typed column of a root field.
pub fn column(name: &str) -> String {
    ident(&format!("f_{name}"))
}

/// The Postgres type of a kind's typed column. Binary has no column.
pub fn column_type(kind: FieldKind) -> Option<&'static str> {
    match kind {
        FieldKind::Keyword | FieldKind::Text => Some("text"),
        FieldKind::Long => Some("bigint"),
        FieldKind::Double => Some("numeric"),
        FieldKind::Date => Some("timestamptz"),
        FieldKind::Boolean => Some("boolean"),
        FieldKind::Binary => None,
    }
}

/// A timestamp from a JSON text value. A bare date is midnight UTC, not the session zone. Digit text is epoch millis:
/// child dates indexed before 2026-10-19 were stored as the millis text a script sent, not as an ISO time.
pub fn ts_expr(text_expr: &str) -> String {
    format!(
        "(CASE WHEN {t} ~ '^-?[0-9]+$' THEN to_timestamp(({t})::bigint / 1000.0) \
         WHEN length({t}) = 10 THEN ({t} || 'T00:00:00Z')::timestamptz ELSE ({t})::timestamptz END)",
        t = text_expr
    )
}

/// A typed value of `key` in the JSON object `object_expr`.
pub fn element_value(object_expr: &str, key: &str, kind: FieldKind) -> String {
    let raw = format!("({object_expr} ->> {})", literal(key));
    match kind {
        FieldKind::Keyword | FieldKind::Text | FieldKind::Binary => raw,
        FieldKind::Long | FieldKind::Double => format!("{raw}::numeric"),
        FieldKind::Date => ts_expr(&raw),
        FieldKind::Boolean => format!("{raw}::boolean"),
    }
}

/// Where field names resolve.
#[derive(Clone)]
enum Scope<'a> {
    Root,
    /// Inside a child array element bound to `var`. `prefix` is the dotted path from the root,
    /// with a trailing dot, stripped from full names.
    Element {
        var: String,
        object: &'a ObjectMapping,
        prefix: String,
    },
}

/// Where a field landed, for the predicate builder.
pub enum Located {
    /// A value expression and its mapping. `arrays` are the `jsonb_array_elements` sources from
    /// outermost to innermost, each `(var, source_expr)`.
    Field {
        arrays: Vec<(String, String)>,
        expr: String,
        field: FieldMapping,
    },
    Missing,
}

pub struct Compiler<'a> {
    pub mapping: &'a DocumentMapping,
    counter: Cell<usize>,
    text: TextSetup,
    documents: Option<&'a [std::sync::Arc<DocumentMapping>]>,
}

impl<'a> Compiler<'a> {
    pub fn new(mapping: &'a DocumentMapping) -> Self {
        Self::with_text(mapping, &TextSetup::default())
    }

    /// A compiler for a store made with `text`.
    pub fn with_text(mapping: &'a DocumentMapping, text: &TextSetup) -> Self {
        Self {
            mapping,
            counter: Cell::new(0),
            text: text.clone(),
            documents: None,
        }
    }

    pub fn with_documents(mut self, documents: Option<&'a [std::sync::Arc<DocumentMapping>]>) -> Self {
        self.documents = documents;
        self
    }

    /// The root `title` field, when it has a typed text column the derived title is made from.
    pub fn title_field(&self) -> Option<&'a str> {
        let name = self.mapping.display.role("title")?;
        let field = self.mapping.root.fields.get(name)?;
        field.kind.is_textual().then_some(name)
    }

    fn var(&self) -> String {
        let next = self.counter.get();
        self.counter.set(next + 1);
        format!("e{next}")
    }

    /// The `WHERE` expression for a query at the document root.
    ///
    /// # Errors
    ///
    /// Returns an error when a value cannot be read for its field, or a child path is not an array.
    pub fn where_sql(&self, query: &Query) -> Result<Sql, Error> {
        if let Some(documents) = self.documents {
            let mut sql = Sql::new("(");
            for (index, mapping) in documents.iter().enumerate() {
                if index > 0 {
                    sql.push(" OR ");
                }
                sql.push(&format!("(__document = {} AND (", literal(&mapping.document_id)));
                sql.append(Compiler::with_text(mapping, &self.text).where_sql(query)?);
                sql.push("))");
            }
            sql.push(")");
            return Ok(sql);
        }
        self.compile(query, &Scope::Root)
    }

    /// Locate a field from the root, for sorts and aggregations.
    pub fn locate_root(&self, name: &str) -> Located {
        self.locate(&Scope::Root, name)
    }

    fn compile(&self, query: &Query, scope: &Scope<'a>) -> Result<Sql, Error> {
        match query {
            Query::MatchAll => Ok(Sql::new("TRUE")),
            // The log viewer's string: field:term, ranges, AND/OR/NOT, groups and phrases are in the facade grammar.
            Query::Lucene(text) => self.compile(&crate::search::parse::parse_query_string(text)?, scope),
            Query::Near {
                terms,
                fields,
                distance,
            } => self.near(scope, terms, fields, *distance),
            Query::Fuzzy { terms } => Ok(self.fuzzy(scope, terms)),
            Query::Suggest { text } => Ok(self.suggest(scope, text)),
            Query::Text { terms, fields, kind } => self.text(scope, terms, fields, *kind),
            Query::Field(field)
                if matches!(scope, Scope::Root) && field.name == crate::search::query::DOCUMENT_FIELD =>
            {
                let matched = field.document_match(&self.mapping.document_id).unwrap_or(false);
                Ok(Sql::new(if matched { "TRUE" } else { "FALSE" }))
            }
            Query::Field(field) => {
                let positive = self.field(scope, field)?;
                if field.negate {
                    let mut out = Sql::new("NOT ");
                    out.append(positive);
                    Ok(out)
                } else {
                    Ok(positive)
                }
            }
            Query::Bool {
                all,
                any,
                none,
                min_any,
            } => {
                let mut parts: Vec<Sql> = Vec::new();
                for item in all {
                    parts.push(self.compile(item, scope)?);
                }
                for item in none {
                    let mut out = Sql::new("NOT ");
                    out.append(self.compile(item, scope)?);
                    parts.push(out);
                }
                let min = effective_min_any(all, any, *min_any) as usize;
                if min > 0 && !any.is_empty() {
                    if min > any.len() {
                        parts.push(Sql::new("FALSE"));
                    } else if min == 1 {
                        let mut out = Sql::new("(");
                        for (index, item) in any.iter().enumerate() {
                            if index > 0 {
                                out.push(" OR ");
                            }
                            out.append(self.compile(item, scope)?);
                        }
                        out.push(")");
                        parts.push(out);
                    } else {
                        let mut out = Sql::new("((");
                        for (index, item) in any.iter().enumerate() {
                            if index > 0 {
                                out.push(" + ");
                            }
                            out.push("(");
                            out.append(self.compile(item, scope)?);
                            out.push(")::int");
                        }
                        out.push(&format!(") >= {min})"));
                        parts.push(out);
                    }
                }
                if parts.is_empty() {
                    return Ok(Sql::new("TRUE"));
                }
                let mut out = Sql::new("(");
                for (index, part) in parts.into_iter().enumerate() {
                    if index > 0 {
                        out.push(" AND ");
                    }
                    out.append(part);
                }
                out.push(")");
                Ok(out)
            }
            Query::Child { path, query } => self.child(scope, path, query),
        }
    }

    fn child(&self, scope: &Scope<'a>, path: &str, query: &Query) -> Result<Sql, Error> {
        let (base_expr, object, prefix) = match scope {
            Scope::Root => ("body".to_string(), &self.mapping.root, String::new()),
            Scope::Element { var, object, prefix } => (var.clone(), *object, prefix.clone()),
        };
        let relative = path.strip_prefix(prefix.as_str()).unwrap_or(path);
        let mut current = object;
        let mut source = base_expr;
        let mut vars = Vec::new();
        for part in relative.split('.') {
            let Some(child) = current.children.get(part) else {
                return Ok(Sql::new("FALSE"));
            };
            let var = self.var();
            vars.push((
                var.clone(),
                format!("jsonb_array_elements({source} -> {})", literal(part)),
            ));
            source = var;
            current = child;
        }
        let inner_scope = Scope::Element {
            var: source,
            object: current,
            prefix: format!("{prefix}{relative}."),
        };
        let inner = self.compile(query, &inner_scope)?;
        Ok(exists(&vars, inner))
    }

    fn locate(&self, scope: &Scope<'a>, name: &str) -> Located {
        let name = name.strip_suffix(".keyword").unwrap_or(name);
        let (base_expr, object, prefix, root) = match scope {
            Scope::Root => {
                match name {
                    "_document" if self.documents.is_some() => {
                        return Located::Field {
                            arrays: Vec::new(),
                            expr: "__document".into(),
                            field: FieldMapping {
                                kind: FieldKind::Keyword,
                                sortable: true,
                            },
                        }
                    }
                    "_id" => {
                        return Located::Field {
                            arrays: Vec::new(),
                            expr: "doc_id".to_string(),
                            field: FieldMapping {
                                kind: FieldKind::Keyword,
                                sortable: true,
                            },
                        }
                    }
                    // A log index has its own `@timestamp`, the event time; the shipper's index time is later.
                    "@timestamp" if self.mapping.root.fields.contains_key(name) => {}
                    "@timestamp" | "_timestamp" | "indexed_at" => {
                        return Located::Field {
                            arrays: Vec::new(),
                            expr: "indexed_at".to_string(),
                            field: FieldMapping {
                                kind: FieldKind::Date,
                                sortable: true,
                            },
                        }
                    }
                    _ => {}
                }
                ("body".to_string(), &self.mapping.root, "", true)
            }
            Scope::Element { var, object, prefix } => (var.clone(), *object, prefix.as_str(), false),
        };
        let relative = name.strip_prefix(prefix).unwrap_or(name);
        if let Some(field) = object.fields.get(relative) {
            let expr = if root {
                column(relative)
            } else {
                element_value(&base_expr, relative, field.kind)
            };
            return Located::Field {
                arrays: Vec::new(),
                expr,
                field: *field,
            };
        }
        let parts: Vec<&str> = relative.split('.').collect();
        let Some((last, chain)) = parts.split_last() else {
            return Located::Missing;
        };
        let mut current = object;
        let mut source = base_expr;
        let mut arrays = Vec::new();
        for part in chain {
            let Some(child) = current.children.get(*part) else {
                return Located::Missing;
            };
            let var = self.var();
            arrays.push((
                var.clone(),
                format!("jsonb_array_elements({source} -> {})", literal(part)),
            ));
            source = var;
            current = child;
        }
        match current.fields.get(*last) {
            Some(field) if !arrays.is_empty() => Located::Field {
                expr: element_value(&source, last, field.kind),
                arrays,
                field: *field,
            },
            _ => Located::Missing,
        }
    }

    /// A predicate on a field: the leaf, wrapped in `EXISTS` for each child array on its path.
    fn on_field(
        &self,
        scope: &Scope<'a>,
        name: &str,
        leaf: impl FnOnce(&str, FieldMapping) -> Result<Sql, Error>,
    ) -> Result<Sql, Error> {
        match self.locate(scope, name) {
            Located::Missing => Ok(Sql::new("FALSE")),
            Located::Field { arrays, expr, field } => {
                let mut inner = Sql::new("COALESCE(");
                inner.append(leaf(&expr, field)?);
                inner.push(", FALSE)");
                if arrays.is_empty() {
                    Ok(inner)
                } else {
                    Ok(exists(&arrays, inner))
                }
            }
        }
    }

    fn text(&self, scope: &Scope<'a>, terms: &str, fields: &TextFields, kind: TextMatch) -> Result<Sql, Error> {
        match fields {
            TextFields::All => {
                let vector = match scope {
                    Scope::Root => "all_text".to_string(),
                    Scope::Element { var, .. } => self.element_vector(var),
                };
                Ok(vector_match(&self.text.config, &vector, terms, kind))
            }
            TextFields::Only(names) => {
                let mut parts = Vec::new();
                for name in names {
                    parts.push(self.on_field(scope, name, |expr, field| {
                        Ok(text_leaf(&self.text.config, expr, field.kind, terms, kind))
                    })?);
                }
                Ok(or_all(parts))
            }
        }
    }

    fn element_vector(&self, var: &str) -> String {
        format!(
            "jsonb_to_tsvector({}, {var}, '[\"string\",\"numeric\"]')",
            literal(&self.text.config)
        )
    }

    /// `Near`: a text query for the words (so the GIN index narrows the rows), then one position of
    /// each word, in order, inside a window of `distance` extra words.
    fn near(&self, scope: &Scope<'a>, terms: &str, fields: &TextFields, distance: u32) -> Result<Sql, Error> {
        let list = words(terms);
        if list.is_empty() {
            return Ok(Sql::new("TRUE"));
        }
        // Each text field on its own: all_text leaves too small a gap between stored values for a
        // window, so a window there could run from one value into the next.
        let names: Vec<String> = match fields {
            TextFields::All => {
                let (object, prefix) = match scope {
                    Scope::Root => (&self.mapping.root, String::new()),
                    Scope::Element { object, prefix, .. } => (*object, prefix.clone()),
                };
                let mut names = Vec::new();
                text_field_names(object, &prefix, &mut names);
                names
            }
            TextFields::Only(names) => names.clone(),
        };
        let mut parts = Vec::new();
        for name in &names {
            parts.push(self.on_field(scope, name, |expr, field| {
                if field.kind == FieldKind::Text {
                    let vector = format!("to_tsvector({}, {expr})", literal(&self.text.config));
                    Ok(self.near_leaf(&vector, &list, distance))
                } else {
                    Ok(Sql::new("FALSE"))
                }
            })?);
        }
        Ok(or_all(parts))
    }

    fn near_leaf(&self, vector: &str, list: &[String], distance: u32) -> Sql {
        let config = literal(&self.text.config);
        let mut out = Sql::new(format!("({vector} @@ to_tsquery({config}, "));
        out.param(Value::String(list.join(" & "))).push(")");
        if list.len() == 1 {
            out.push(")");
            return out;
        }
        if list.len() == 2 && distance < NEAR_PHRASE_LIMIT {
            // Two words: "within distance" is the second word 1 to distance + 1 positions after the first, which
            // the phrase operators check in the GIN-indexed match itself, without reading positions row by row.
            let (first, second) = (&list[0], &list[1]);
            let alternatives: Vec<String> = (1..=distance + 1)
                .map(|gap| format!("{first} <{gap}> {second}"))
                .collect();
            let mut out = Sql::new(format!("({vector} @@ to_tsquery({config}, "));
            out.param(Value::String(alternatives.join(" | "))).push("))");
            return out;
        }
        let names: Vec<(String, String)> = list.iter().map(|_| (self.var(), self.var())).collect();
        out.push(" AND EXISTS (SELECT 1 FROM ");
        for (index, (lexeme, position)) in names.iter().enumerate() {
            if index > 0 {
                out.push(", ");
            }
            out.push(&format!(
                "unnest({vector}) AS {lexeme}, unnest({lexeme}.positions) AS {position}"
            ));
        }
        // Each word is checked against the window as the join adds it, so a partial match that is already too
        // wide is dropped there and the join does not grow as the product of the words' positions.
        let first = &names[0].1;
        out.push(" WHERE ");
        for (index, ((lexeme, position), word)) in names.iter().zip(list).enumerate() {
            if index > 0 {
                out.push(&format!(
                    " AND {position} > {} AND {position} - {first} <= {}",
                    names[index - 1].1,
                    u64::from(distance) + index as u64
                ));
                out.push(" AND ");
            }
            // The word as the configuration stores it.
            out.push(&format!(
                "{lexeme}.lexeme = (SELECT t.lexeme FROM unnest(to_tsvector({config}, "
            ));
            out.param(Value::string(word)).push(")) AS t LIMIT 1)");
        }
        out.push("))");
        out
    }

    /// `Fuzzy`: the folded title's word similarity to the terms is over `pg_trgm`'s threshold.
    fn fuzzy(&self, scope: &Scope<'a>, terms: &str) -> Sql {
        if !matches!(scope, Scope::Root) || self.title_field().is_none() || terms.trim().is_empty() {
            return Sql::new("FALSE");
        }
        let mut out = Sql::new(format!("{}(", self.text.fold));
        out.param(Value::string(terms.trim()))
            .push(&format!(") <% {NORM_TITLE}"));
        out
    }

    /// `Suggest`: the folded title starts with the text, or each word starts a title word.
    fn suggest(&self, scope: &Scope<'a>, text: &str) -> Sql {
        let Some(title) = self.title_field().filter(|_| matches!(scope, Scope::Root)) else {
            return Sql::new("FALSE");
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Sql::new("TRUE");
        }
        let mut out = Sql::new(format!("({NORM_TITLE} LIKE {}(", self.text.fold));
        out.param(Value::String(format!("{}%", like_escape(trimmed)))).push(")");
        if let Some(prefix) = tsquery(&self.text.config, trimmed, TextMatch::Prefix) {
            out.push(&format!(
                " OR to_tsvector({}, {}) @@ ",
                literal(&self.text.config),
                column(title)
            ));
            out.append(prefix);
        }
        out.push(")");
        out
    }

    /// Score terms beside the text rank: closeness for `Fuzzy`, and a title start over a word start
    /// for `Suggest`. Root clauses only, and not under a negation.
    pub fn extra_score(&self, query: &Query) -> Vec<Sql> {
        let mut out = Vec::new();
        self.extra_parts(query, false, &mut out);
        out
    }

    fn extra_parts(&self, query: &Query, negated: bool, out: &mut Vec<Sql>) {
        match query {
            Query::Fuzzy { terms } if !negated && self.title_field().is_some() && !terms.trim().is_empty() => {
                let mut part = Sql::new(format!("COALESCE(word_similarity({}(", self.text.fold));
                part.param(Value::string(terms.trim()))
                    .push(&format!("), {NORM_TITLE}), 0)"));
                out.push(part);
            }
            Query::Suggest { text } if !negated && self.title_field().is_some() && !text.trim().is_empty() => {
                let mut part = Sql::new(format!("(CASE WHEN {NORM_TITLE} LIKE {}(", self.text.fold));
                part.param(Value::String(format!("{}%", like_escape(text.trim()))))
                    .push(") THEN 1.0 ELSE 0.5 END)");
                out.push(part);
            }
            Query::Bool { all, any, none, .. } => {
                for item in all.iter().chain(any) {
                    self.extra_parts(item, negated, out);
                }
                for item in none {
                    self.extra_parts(item, !negated, out);
                }
            }
            _ => {}
        }
    }

    fn field(&self, scope: &Scope<'a>, query: &FieldQuery) -> Result<Sql, Error> {
        let list = self.root_list(scope, &query.name);
        self.on_field(scope, &query.name, |expr, field| {
            let leaf = field_leaf(&self.text.config, expr, field.kind, query)?;
            Ok(match list {
                Some(list) if field.kind == FieldKind::Keyword && matches!(query.op, FieldOp::Eq | FieldOp::In) => {
                    or_list_element(leaf, &list, query)
                }
                _ => leaf,
            })
        })
    }

    /// The body value of a root field, for a document that holds a list there (a keyword field such as a document's
    /// tag ids): the typed column of a list is null, so `eq` and `in` also look in the list, as an Elastic term query
    /// matches any element of an array.
    fn root_list(&self, scope: &Scope<'a>, name: &str) -> Option<String> {
        let name = name.strip_suffix(".keyword").unwrap_or(name);
        (matches!(scope, Scope::Root) && self.mapping.root.fields.contains_key(name))
            .then(|| format!("(body -> {})", literal(name)))
    }

    /// The query whose `ts_rank` orders hits: the positive text clauses, OR-ed.
    pub fn rank_query(&self, query: &Query) -> Option<Sql> {
        let mut parts = Vec::new();
        rank_parts(&self.text.config, query, false, &mut parts);
        if parts.is_empty() {
            return None;
        }
        let mut out = Sql::new("(");
        for (index, part) in parts.into_iter().enumerate() {
            if index > 0 {
                out.push(" || ");
            }
            out.append(part);
        }
        out.push(")");
        Some(out)
    }
}

fn rank_parts(config: &str, query: &Query, negated: bool, out: &mut Vec<Sql>) {
    match query {
        Query::Text { terms, kind, .. } if !negated => {
            if let Some(query) = tsquery(config, terms, *kind) {
                out.push(query);
            }
        }
        Query::Near { terms, .. } if !negated => {
            let list = words(terms);
            if !list.is_empty() {
                let mut part = Sql::new(format!("to_tsquery({}, ", literal(config)));
                part.param(Value::String(list.join(" & "))).push(")");
                out.push(part);
            }
        }
        Query::Bool { all, any, none, .. } => {
            for item in all.iter().chain(any) {
                rank_parts(config, item, negated, out);
            }
            for item in none {
                rank_parts(config, item, !negated, out);
            }
        }
        Query::Child { query, .. } => rank_parts(config, query, negated, out),
        _ => {}
    }
}

/// Full names of the text fields of `object` and its children, `prefix` first.
fn text_field_names(object: &ObjectMapping, prefix: &str, out: &mut Vec<String>) {
    for (name, field) in &object.fields {
        if field.kind == FieldKind::Text {
            out.push(format!("{prefix}{name}"));
        }
    }
    for (name, child) in &object.children {
        text_field_names(child, &format!("{prefix}{name}."), out);
    }
}

fn exists(arrays: &[(String, String)], inner: Sql) -> Sql {
    let mut out = Sql::new("EXISTS (SELECT 1 FROM ");
    for (index, (var, source)) in arrays.iter().enumerate() {
        if index > 0 {
            out.push(", ");
        }
        out.push(&format!("{source} AS {var}"));
    }
    out.push(" WHERE ");
    out.append(inner);
    out.push(")");
    out
}

fn or_all(parts: Vec<Sql>) -> Sql {
    match parts.len() {
        0 => Sql::new("FALSE"),
        1 => parts.into_iter().next().unwrap_or_default(),
        _ => {
            let mut out = Sql::new("(");
            for (index, part) in parts.into_iter().enumerate() {
                if index > 0 {
                    out.push(" OR ");
                }
                out.append(part);
            }
            out.push(")");
            out
        }
    }
}

/// Lower-cased alphanumeric words.
fn words(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// A `tsquery` for term, prefix, and phrase text. Contains and wildcard have none.
fn tsquery(config: &str, terms: &str, kind: TextMatch) -> Option<Sql> {
    let config = literal(config);
    match kind {
        TextMatch::Term => {
            let mut out = Sql::new(format!("plainto_tsquery({config}, "));
            out.param(Value::string(terms)).push(")");
            Some(out)
        }
        TextMatch::Phrase => {
            let mut out = Sql::new(format!("phraseto_tsquery({config}, "));
            out.param(Value::string(terms)).push(")");
            Some(out)
        }
        TextMatch::Prefix => {
            let list = words(terms);
            if list.is_empty() {
                return None;
            }
            let text = list
                .iter()
                .map(|word| format!("{word}:*"))
                .collect::<Vec<_>>()
                .join(" & ");
            let mut out = Sql::new(format!("to_tsquery({config}, "));
            out.param(Value::String(text)).push(")");
            Some(out)
        }
        TextMatch::Contains | TextMatch::Wildcard => None,
    }
}

/// `LIKE` escape: `\`, `%`, and `_` are literal.
fn like_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 4);
    for ch in text.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// A parsed wildcard (`*`, `?`, `\` escapes) as a `LIKE` pattern.
fn wildcard_like(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 4);
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                if let Some(next) = chars.next() {
                    out.push_str(&like_escape(&next.to_string()));
                }
            }
            '*' => out.push('%'),
            '?' => out.push('_'),
            other => out.push_str(&like_escape(&other.to_string())),
        }
    }
    out
}

fn vector_match(config: &str, vector: &str, terms: &str, kind: TextMatch) -> Sql {
    match kind {
        TextMatch::Term | TextMatch::Prefix | TextMatch::Phrase => match tsquery(config, terms, kind) {
            Some(query) => {
                let mut out = Sql::new(format!("{vector} @@ "));
                out.append(query);
                out
            }
            None => Sql::new("TRUE"),
        },
        TextMatch::Contains | TextMatch::Wildcard => {
            let pattern = if kind == TextMatch::Contains {
                format!("%{}%", like_escape(&terms.to_lowercase()))
            } else {
                wildcard_like(&terms.to_lowercase())
            };
            let mut out = Sql::new(format!(
                "EXISTS (SELECT 1 FROM unnest(tsvector_to_array({vector})) AS w WHERE w LIKE "
            ));
            out.param(Value::String(pattern)).push(")");
            out
        }
    }
}

fn text_leaf(config: &str, expr: &str, kind: FieldKind, terms: &str, matching: TextMatch) -> Sql {
    match kind {
        FieldKind::Text => match matching {
            TextMatch::Contains => {
                let mut out = Sql::new(format!("{expr} ILIKE "));
                out.param(Value::String(format!("%{}%", like_escape(terms))));
                out
            }
            _ => vector_match(
                config,
                &format!("to_tsvector({}, {expr})", literal(config)),
                terms,
                matching,
            ),
        },
        FieldKind::Keyword => {
            let (op, value) = match matching {
                TextMatch::Term | TextMatch::Phrase => ("=", terms.to_string()),
                TextMatch::Prefix => ("LIKE", format!("{}%", like_escape(terms))),
                TextMatch::Contains => ("LIKE", format!("%{}%", like_escape(terms))),
                TextMatch::Wildcard => ("LIKE", wildcard_like(terms)),
            };
            let mut out = Sql::new(format!("{expr} {op} "));
            out.param(Value::String(value));
            out
        }
        FieldKind::Date if matches!(matching, TextMatch::Term | TextMatch::Phrase) => {
            // A bare date matches the whole UTC day, as an Elastic date term rounds to the day.
            let trimmed = terms.trim();
            match parse_date_bound(&Value::string(trimmed)) {
                Some(start) if trimmed.len() == 10 => {
                    let end = start.checked_add(jiff::SignedDuration::from_hours(24)).unwrap_or(start);
                    let mut out = Sql::new(format!("({expr} >= "));
                    out.param(Value::Timestamp(start))
                        .push(&format!("::timestamptz AND {expr} < "))
                        .param(Value::Timestamp(end))
                        .push("::timestamptz)");
                    out
                }
                Some(at) => {
                    let mut out = Sql::new(format!("{expr} = "));
                    out.param(Value::Timestamp(at)).push("::timestamptz");
                    out
                }
                None => Sql::new("FALSE"),
            }
        }
        _ if matches!(matching, TextMatch::Term | TextMatch::Phrase) => {
            match typed_param(kind, &Value::string(terms)) {
                Some((value, cast)) => {
                    let mut out = Sql::new(format!("{expr} = "));
                    out.param(value).push(cast);
                    out
                }
                None => Sql::new("FALSE"),
            }
        }
        _ => Sql::new("FALSE"),
    }
}

fn field_leaf(config: &str, expr: &str, kind: FieldKind, query: &FieldQuery) -> Result<Sql, Error> {
    Ok(match query.op {
        FieldOp::Exists => Sql::new(format!("{expr} IS NOT NULL")),
        FieldOp::Eq => {
            if kind == FieldKind::Text {
                text_leaf(config, expr, kind, &to_plain_string(&query.value), TextMatch::Term)
            } else {
                compare(expr, kind, "=", &query.value)
            }
        }
        FieldOp::In => {
            let values: Vec<&Value> = if query.values.is_empty() && !query.value.is_null() {
                vec![&query.value]
            } else {
                query.values.iter().collect()
            };
            if kind == FieldKind::Text {
                return Ok(or_all(
                    values
                        .into_iter()
                        .map(|value| text_leaf(config, expr, kind, &to_plain_string(value), TextMatch::Term))
                        .collect(),
                ));
            }
            let mut out = Sql::new(format!("{expr} IN ("));
            let mut count = 0;
            for value in values {
                if let Some((param, cast)) = typed_param(kind, value) {
                    if count > 0 {
                        out.push(", ");
                    }
                    out.param(param).push(cast);
                    count += 1;
                }
            }
            if count == 0 {
                return Ok(Sql::new("FALSE"));
            }
            out.push(")");
            out
        }
        FieldOp::Prefix => text_leaf(config, expr, kind, &to_plain_string(&query.value), TextMatch::Prefix),
        FieldOp::Wildcard => text_leaf(config, expr, kind, &to_plain_string(&query.value), TextMatch::Wildcard),
        FieldOp::Range => {
            let mut parts = Vec::new();
            for (op, bound) in [
                (">", &query.gt),
                (">=", &query.gte),
                ("<", &query.lt),
                ("<=", &query.lte),
            ] {
                if let Some(bound) = bound {
                    parts.push(compare(expr, kind, op, bound));
                }
            }
            if parts.is_empty() {
                return Ok(Sql::new(format!("{expr} IS NOT NULL")));
            }
            let mut out = Sql::new("(");
            for (index, part) in parts.into_iter().enumerate() {
                if index > 0 {
                    out.push(" AND ");
                }
                out.append(part);
            }
            out.push(")");
            out
        }
    })
}

/// `COALESCE(leaf, list @> [value] OR ...)`: the column's answer, or when the column is null (the document holds a list,
/// or nothing) whether the list in the body has one of the values. A body value that is not a list contains no array, so
/// a scalar never matches through this arm.
fn or_list_element(leaf: Sql, list: &str, query: &FieldQuery) -> Sql {
    let values: Vec<&Value> = match query.op {
        FieldOp::In if !query.values.is_empty() => query.values.iter().collect(),
        _ => vec![&query.value],
    };
    let values: Vec<&Value> = values.into_iter().filter(|value| !value.is_null()).collect();
    if values.is_empty() {
        return leaf;
    }
    let mut out = Sql::new("COALESCE(");
    out.append(leaf);
    out.push(", ");
    for (index, value) in values.into_iter().enumerate() {
        if index > 0 {
            out.push(" OR ");
        }
        out.push(&format!("{list} @> jsonb_build_array("));
        out.param(Value::String(to_plain_string(value))).push("::text)");
    }
    out.push(")");
    out
}

fn compare(expr: &str, kind: FieldKind, op: &str, value: &Value) -> Sql {
    match typed_param(kind, value) {
        Some((param, cast)) => {
            let mut out = Sql::new(format!("{expr} {op} "));
            out.param(param).push(cast);
            out
        }
        None => Sql::new("FALSE"),
    }
}

/// A parameter for a kind and the cast that goes after it. `None` when the value cannot be one.
pub fn typed_param(kind: FieldKind, value: &Value) -> Option<(Value, &'static str)> {
    if value.is_null() {
        return None;
    }
    match kind {
        FieldKind::Keyword | FieldKind::Text | FieldKind::Binary => Some((Value::String(to_plain_string(value)), "")),
        FieldKind::Long | FieldKind::Double => match basic_convert(value, "BigDecimal").value {
            decimal @ Value::Decimal(_) => Some((decimal, "::numeric")),
            _ => None,
        },
        FieldKind::Date => parse_date_bound(value).map(|at| (Value::Timestamp(at), "::timestamptz")),
        FieldKind::Boolean => match to_plain_string(value).to_ascii_lowercase().as_str() {
            "true" | "y" => Some((Value::Bool(true), "::boolean")),
            "false" | "n" => Some((Value::Bool(false), "::boolean")),
            _ => None,
        },
    }
}

/// Above this many positions between two words, `Near` checks positions row by row instead of an OR of phrase
/// operators.
const NEAR_PHRASE_LIMIT: u32 = 64;

/// A date bound: a timestamp, a date (midnight UTC), epoch millis, ISO-8601, `now`, or the
/// Moqui `yyyy-MM-dd HH:mm:ss.SSS` form read as UTC.
pub fn parse_date_bound(value: &Value) -> Option<Timestamp> {
    match value {
        Value::Timestamp(at) => Some(*at),
        Value::Date(date) => date.to_zoned(TimeZone::UTC).ok().map(|zoned| zoned.timestamp()),
        Value::I32(millis) => Timestamp::from_millisecond(i64::from(*millis)).ok(),
        Value::I64(millis) => Timestamp::from_millisecond(*millis).ok(),
        Value::Decimal(_) | Value::F64(_) => {
            let text = to_plain_string(value);
            text.split('.')
                .next()?
                .parse::<i64>()
                .ok()
                .and_then(|millis| Timestamp::from_millisecond(millis).ok())
        }
        Value::String(text) if text.trim() == "now" => Some(Timestamp::now()),
        Value::String(text) => parse_timestamp(text, &TimeZone::UTC),
        _ => None,
    }
}

#[cfg(test)]
#[path = "compile_tests.rs"]
mod tests;
