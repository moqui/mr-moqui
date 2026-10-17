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

//! Condition tree. `EntityFind` builds it. This module renders it and collects the bound values.
//!
//! As Java `FieldValueCondition` does, a null value turns equals, like, in, and between into
//! `IS NULL`, and their negations into `IS NOT NULL`. An empty `In` list is false: `1 = 0`. An
//! empty `NotIn` list is true: `1 = 1`. A part of an `And` or `Or` with nothing to render is
//! dropped, and an `And` or `Or` left with nothing is omitted. An `Or` of no parts is false
//! (`1 = 0`), and `Not` of nothing is false.

use moqui_lib::error::Error;
use moqui_lib::object::Value;

use crate::dialect::Dialect;

use super::ident::quote_ident;
use super::Statement;

/// Entity alias (optional) plus a logical field name. Entity/view builders resolve physical
/// columns; standalone `render_condition` has no model and quotes names verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FieldRef {
    pub entity_alias: Option<String>,
    pub name: String,
    /// SQL written in place of the column. Only the renderers set it, from the model's own
    /// aliases, so no input ever reaches it.
    pub expr: Option<String>,
}

impl FieldRef {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            entity_alias: None,
            name: name.into(),
            expr: None,
        }
    }

    pub fn aliased(entity_alias: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            entity_alias: Some(entity_alias.into()),
            name: name.into(),
            expr: None,
        }
    }

    /// A field that is a SQL expression, such as `SUM("OI"."quantity")`. `name` is its alias.
    pub fn expression(name: impl Into<String>, expr: impl Into<String>) -> Self {
        Self {
            entity_alias: None,
            name: name.into(),
            expr: Some(expr.into()),
        }
    }
}

impl From<&str> for FieldRef {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

impl From<String> for FieldRef {
    fn from(name: String) -> Self {
        Self::new(name)
    }
}

/// One order-by entry.
///
/// Java writes `NULLS LAST` unless told otherwise, on every database that does not set
/// `never-nulls`. `case` is `UPPER` (`Some(true)`) or `LOWER` (`Some(false)`), applied to text fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderBy {
    pub field: FieldRef,
    pub descending: bool,
    pub nulls: Option<Nulls>,
    pub case: Option<bool>,
}

impl OrderBy {
    pub fn asc(field: impl Into<FieldRef>) -> Self {
        Self {
            field: field.into(),
            descending: false,
            nulls: None,
            case: None,
        }
    }

    pub fn desc(field: impl Into<FieldRef>) -> Self {
        Self {
            descending: true,
            ..Self::asc(field)
        }
    }

    pub fn nulls(mut self, nulls: Nulls) -> Self {
        self.nulls = Some(nulls);
        self
    }

    /// Java `FieldOrderOptions`: leading `-` (descending), `+`, and `^` (upper case), then the
    /// field name, then optional ` DESC`/` ASC` and ` NULLS FIRST`/` NULLS LAST`, and
    /// `UPPER(field)` or `LOWER(field)`. Blank text is `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let mut descending = false;
        let mut case = None;
        let mut rest = text.trim_start();
        loop {
            match rest.chars().next() {
                Some('-') => descending = true,
                Some('+') => descending = false,
                Some('^') => case = Some(true),
                Some(' ') => {}
                _ => break,
            }
            rest = &rest[1..];
        }
        let mut name = rest.trim_end().to_string();
        if name.is_empty() {
            return None;
        }
        let mut nulls = None;
        let upper = name.to_ascii_uppercase();
        if upper.ends_with(" NULLS FIRST") {
            nulls = Some(Nulls::First);
            name.truncate(name.len() - 12);
        } else if upper.ends_with(" NULLS LAST") {
            nulls = Some(Nulls::Last);
            name.truncate(name.len() - 11);
        }
        let upper = name.to_ascii_uppercase();
        if upper.ends_with(" DESC") {
            descending = true;
            name.truncate(name.len() - 5);
        } else if upper.ends_with(" ASC") {
            descending = false;
            name.truncate(name.len() - 4);
        }
        if name.contains('(') {
            let upper = name.to_ascii_uppercase();
            if upper.starts_with("UPPER(") {
                case = Some(true);
                name.drain(..6);
            } else if upper.starts_with("LOWER(") {
                case = Some(false);
                name.drain(..6);
            }
            if name.ends_with(')') {
                name.pop();
            }
        }
        let name = name.trim().to_string();
        if name.is_empty() {
            return None;
        }
        Some(Self {
            field: FieldRef::new(name),
            descending,
            nulls,
            case,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nulls {
    First,
    Last,
}

/// Limit and offset, bound as values. Absent means that clause is left out.
///
/// Postgres (`offset-style` absent) writes `LIMIT ? OFFSET ?` whenever a limit is set, and
/// `OFFSET ?` when only an offset is set. MySQL (`offset-style` `limit`) writes `LIMIT ?`, or
/// `LIMIT ?, ?` (offset, then limit) when an offset is set. A MySQL offset without a limit binds
/// the largest limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LimitOffset {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl LimitOffset {
    pub fn is_set(self) -> bool {
        self.limit.is_some() || self.offset.is_some()
    }
}

/// Predicate `EntityFind` will build. Values are not inlined. Each bound value is `?`.
#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    Equals(FieldRef, Value),
    NotEquals(FieldRef, Value),
    Less(FieldRef, Value),
    Greater(FieldRef, Value),
    LessEqual(FieldRef, Value),
    GreaterEqual(FieldRef, Value),
    In(FieldRef, Vec<Value>),
    NotIn(FieldRef, Vec<Value>),
    Between(FieldRef, Value, Value),
    NotBetween(FieldRef, Value, Value),
    Like(FieldRef, Value),
    NotLike(FieldRef, Value),
    IsNull(FieldRef),
    IsNotNull(FieldRef),
    And(Vec<Condition>),
    Or(Vec<Condition>),
    Not(Box<Condition>),
    /// Full text. The SQL comes from the dialect `fts-style` and `fts-config`.
    Matches(FieldRef, Value),
    /// One field against another: `to-field-name` on an `econdition`.
    CompareFields(FieldRef, CompareOp, FieldRef),
    /// A comparison made on `UPPER(field)` and `UPPER(value)` when the field is text. The inner
    /// condition is one comparison, against a value or another field.
    IgnoreCase(Box<Condition>),
    /// A SQL fragment used as written, Java's `WhereCondition` (`makeConditionWhere`). It names tables and columns as
    /// the database does, binds no values, and cannot be checked against a row held in memory.
    Where(String),
    /// `field op ?` for a statement prepared once and bound by the caller on each run. Its slot
    /// in [`Statement::params`] is `Null`; such a caller uses the SQL text only.
    Bound(FieldRef, CompareOp),
}

/// The comparison between two fields, or against a value bound by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Equals,
    NotEquals,
    Less,
    Greater,
    LessEqual,
    GreaterEqual,
}

impl CompareOp {
    pub(crate) fn sql(self) -> &'static str {
        match self {
            Self::Equals => "=",
            Self::NotEquals => "<>",
            Self::Less => "<",
            Self::Greater => ">",
            Self::LessEqual => "<=",
            Self::GreaterEqual => ">=",
        }
    }

    /// An `operator` attribute value. `None` for one that does not compare two fields.
    pub fn from_operator(operator: &str) -> Option<Self> {
        Some(match operator {
            "equals" | "=" => Self::Equals,
            "not-equals" | "!=" | "<>" => Self::NotEquals,
            "less" | "<" => Self::Less,
            "greater" | ">" => Self::Greater,
            "less-equals" | "<=" => Self::LessEqual,
            "greater-equals" | ">=" => Self::GreaterEqual,
            _ => return None,
        })
    }
}

/// A condition from an `operator` attribute value. `in` and `between` take a list; a text value
/// is split on commas, as Java `valueToCollection` does.
pub fn comparison(field: impl Into<FieldRef>, operator: &str, value: Value) -> Result<Condition, Error> {
    let field = field.into();
    let list = |value: Value| -> Vec<Value> {
        match value {
            Value::List(items) => items,
            Value::String(text) => text.split(',').map(Value::string).collect(),
            Value::Null => Vec::new(),
            other => vec![other],
        }
    };
    let bounds = |value: Value| -> Result<(Value, Value), Error> {
        let mut items = list(value).into_iter();
        match (items.next(), items.next()) {
            (Some(low), Some(high)) => Ok((low, high)),
            _ => Err(Error::new(format!("{operator} needs a low and a high value"))),
        }
    };
    Ok(match operator {
        "equals" | "=" | "" => Condition::Equals(field, value),
        "not-equals" | "!=" | "<>" => Condition::NotEquals(field, value),
        "less" | "<" => Condition::Less(field, value),
        "greater" | ">" => Condition::Greater(field, value),
        "less-equals" | "<=" => Condition::LessEqual(field, value),
        "greater-equals" | ">=" => Condition::GreaterEqual(field, value),
        "like" => Condition::Like(field, value),
        "not-like" => Condition::NotLike(field, value),
        "in" if value.is_null() => Condition::IsNull(field),
        "in" => Condition::In(field, list(value)),
        "not-in" if value.is_null() => Condition::IsNotNull(field),
        "not-in" => Condition::NotIn(field, list(value)),
        "between" if value.is_null() => Condition::IsNull(field),
        "between" => {
            let (low, high) = bounds(value)?;
            Condition::Between(field, low, high)
        }
        "not-between" if value.is_null() => Condition::IsNotNull(field),
        "not-between" => {
            let (low, high) = bounds(value)?;
            Condition::NotBetween(field, low, high)
        }
        "is-null" => Condition::IsNull(field),
        "is-not-null" => Condition::IsNotNull(field),
        other => return Err(Error::new(format!("unknown condition operator {other}"))),
    })
}

impl Condition {
    pub fn equals(field: impl Into<FieldRef>, value: Value) -> Self {
        Self::Equals(field.into(), value)
    }

    pub fn not_equals(field: impl Into<FieldRef>, value: Value) -> Self {
        Self::NotEquals(field.into(), value)
    }

    pub fn less(field: impl Into<FieldRef>, value: Value) -> Self {
        Self::Less(field.into(), value)
    }

    pub fn greater(field: impl Into<FieldRef>, value: Value) -> Self {
        Self::Greater(field.into(), value)
    }

    pub fn less_equal(field: impl Into<FieldRef>, value: Value) -> Self {
        Self::LessEqual(field.into(), value)
    }

    pub fn greater_equal(field: impl Into<FieldRef>, value: Value) -> Self {
        Self::GreaterEqual(field.into(), value)
    }

    pub fn is_in(field: impl Into<FieldRef>, values: Vec<Value>) -> Self {
        Self::In(field.into(), values)
    }

    pub fn not_in(field: impl Into<FieldRef>, values: Vec<Value>) -> Self {
        Self::NotIn(field.into(), values)
    }

    pub fn between(field: impl Into<FieldRef>, low: Value, high: Value) -> Self {
        Self::Between(field.into(), low, high)
    }

    pub fn not_between(field: impl Into<FieldRef>, low: Value, high: Value) -> Self {
        Self::NotBetween(field.into(), low, high)
    }

    pub fn like(field: impl Into<FieldRef>, value: Value) -> Self {
        Self::Like(field.into(), value)
    }

    pub fn not_like(field: impl Into<FieldRef>, value: Value) -> Self {
        Self::NotLike(field.into(), value)
    }

    pub fn is_null(field: impl Into<FieldRef>) -> Self {
        Self::IsNull(field.into())
    }

    pub fn is_not_null(field: impl Into<FieldRef>) -> Self {
        Self::IsNotNull(field.into())
    }

    pub fn and(conditions: Vec<Condition>) -> Self {
        Self::And(conditions)
    }

    pub fn or(conditions: Vec<Condition>) -> Self {
        Self::Or(conditions)
    }

    pub fn negate(condition: Condition) -> Self {
        Self::Not(Box::new(condition))
    }

    pub fn matches(field: impl Into<FieldRef>, value: Value) -> Self {
        Self::Matches(field.into(), value)
    }

    pub fn compare_fields(left: impl Into<FieldRef>, op: CompareOp, right: impl Into<FieldRef>) -> Self {
        Self::CompareFields(left.into(), op, right.into())
    }

    /// `condition` compared without regard to case.
    pub fn ignore_case(condition: Condition) -> Self {
        Self::IgnoreCase(Box::new(condition))
    }

    /// A SQL fragment used as written.
    pub fn where_sql(sql: impl Into<String>) -> Self {
        Self::Where(sql.into())
    }

    /// `field op ?`, bound by the caller.
    pub fn bound(field: impl Into<FieldRef>, op: CompareOp) -> Self {
        Self::Bound(field.into(), op)
    }

    /// Rewrite every field in the tree.
    #[must_use]
    pub fn map_fields(self, change: &mut impl FnMut(FieldRef) -> FieldRef) -> Self {
        match self {
            Self::Equals(field, value) => Self::Equals(change(field), value),
            Self::NotEquals(field, value) => Self::NotEquals(change(field), value),
            Self::Less(field, value) => Self::Less(change(field), value),
            Self::Greater(field, value) => Self::Greater(change(field), value),
            Self::LessEqual(field, value) => Self::LessEqual(change(field), value),
            Self::GreaterEqual(field, value) => Self::GreaterEqual(change(field), value),
            Self::In(field, values) => Self::In(change(field), values),
            Self::NotIn(field, values) => Self::NotIn(change(field), values),
            Self::Between(field, low, high) => Self::Between(change(field), low, high),
            Self::NotBetween(field, low, high) => Self::NotBetween(change(field), low, high),
            Self::Like(field, value) => Self::Like(change(field), value),
            Self::NotLike(field, value) => Self::NotLike(change(field), value),
            Self::IsNull(field) => Self::IsNull(change(field)),
            Self::IsNotNull(field) => Self::IsNotNull(change(field)),
            Self::Matches(field, value) => Self::Matches(change(field), value),
            Self::Bound(field, op) => Self::Bound(change(field), op),
            Self::Where(sql) => Self::Where(sql),
            Self::CompareFields(left, op, right) => {
                let left = change(left);
                Self::CompareFields(left, op, change(right))
            }
            Self::And(list) => Self::And(list.into_iter().map(|child| child.map_fields(change)).collect()),
            Self::Or(list) => Self::Or(list.into_iter().map(|child| child.map_fields(change)).collect()),
            Self::Not(inner) => Self::Not(Box::new(inner.map_fields(change))),
            Self::IgnoreCase(inner) => Self::IgnoreCase(Box::new(inner.map_fields(change))),
        }
    }

    /// True when any field in the tree satisfies `test`.
    pub fn any_field(&self, test: &mut impl FnMut(&FieldRef) -> bool) -> bool {
        let mut found = false;
        self.visit_fields(&mut |field| {
            if !found && test(field) {
                found = true;
            }
        });
        found
    }

    /// Call `visit` on every field in the tree.
    pub fn visit_fields(&self, visit: &mut impl FnMut(&FieldRef)) {
        match self {
            Self::Equals(field, _)
            | Self::NotEquals(field, _)
            | Self::Less(field, _)
            | Self::Greater(field, _)
            | Self::LessEqual(field, _)
            | Self::GreaterEqual(field, _)
            | Self::In(field, _)
            | Self::NotIn(field, _)
            | Self::Between(field, _, _)
            | Self::NotBetween(field, _, _)
            | Self::Like(field, _)
            | Self::NotLike(field, _)
            | Self::IsNull(field)
            | Self::IsNotNull(field)
            | Self::Bound(field, _)
            | Self::Matches(field, _) => visit(field),
            Self::CompareFields(left, _, right) => {
                visit(left);
                visit(right);
            }
            Self::And(list) | Self::Or(list) => {
                for child in list {
                    child.visit_fields(visit);
                }
            }
            Self::Not(inner) | Self::IgnoreCase(inner) => inner.visit_fields(visit),
            Self::Where(_) => {}
        }
    }

    /// `left and right`, either of which may be absent.
    pub fn and_opt(left: Option<Condition>, right: Option<Condition>) -> Option<Condition> {
        match (left, right) {
            (Some(left), Some(right)) => Some(Self::And(vec![left, right])),
            (left, None) => left,
            (None, right) => right,
        }
    }
}

/// Render a condition with field names quoted as written. An empty `And` or `Or` is an empty
/// statement. Ignore-case applies `UPPER` to every field, since no field types are known here.
pub fn render_condition(condition: &Condition, dialect: &Dialect) -> Result<Statement, Error> {
    let columns = |field: &FieldRef| -> Result<Column, Error> { Ok(Column::plain(dialect, field)) };
    let mut out = Writer::new(dialect);
    out.condition(condition, &columns)?;
    Ok(out.finish())
}

/// A field as written in SQL, and whether it is text (`None` when that is not known).
#[derive(Debug, Clone)]
pub(crate) struct Column {
    pub sql: String,
    pub text: Option<bool>,
    /// The field's Java type when the entity or view knows it. A text value compared to a date, time, or number
    /// field is converted to it before binding, as Java's condition does with the field type.
    pub java_type: Option<String>,
    /// The generated full-text column of a `text-fts` field of a plain entity, Java's `{column}_tsv` on Postgres.
    pub fts: Option<String>,
}

impl Column {
    /// The field quoted as written: `"alias"."name"`, or its expression.
    pub(crate) fn plain(dialect: &Dialect, field: &FieldRef) -> Self {
        let mut sql = String::new();
        write_field(&mut sql, dialect, field);
        Self {
            sql,
            text: None,
            java_type: None,
            fts: None,
        }
    }

    fn upper_applies(&self) -> bool {
        self.text != Some(false)
    }

    /// A text value converted to the column's date, time, or number type; anything else as given. Scripts carry
    /// dates as epoch millis text, so that form is read as a moment.
    fn typed(&self, value: Value) -> Value {
        let (Some(java_type), Value::String(text)) = (self.java_type.as_deref(), &value) else {
            return value;
        };
        let simple = java_type.rsplit('.').next().unwrap_or(java_type);
        let temporal = matches!(simple, "Date" | "Timestamp" | "Time");
        let number = matches!(
            simple,
            "BigDecimal" | "BigInteger" | "Long" | "Integer" | "Double" | "Float"
        );
        if !temporal && !number {
            return value;
        }
        let source = match text.trim().parse::<i64>() {
            Ok(millis) if temporal => moqui_lib::object::basic_convert(&Value::I64(millis), "java.sql.Timestamp").value,
            _ => value.clone(),
        };
        match moqui_lib::object::basic_convert(&source, java_type).value {
            Value::String(_) => value,
            converted => converted,
        }
    }
}

/// Maps a field in a condition to its SQL. Each renderer supplies one from its entity or view.
pub(crate) type Columns<'a> = dyn Fn(&FieldRef) -> Result<Column, Error> + 'a;

/// SQL text plus the values bound so far.
pub(crate) struct Writer<'a> {
    pub dialect: &'a Dialect,
    pub sql: String,
    pub params: Vec<Value>,
}

impl<'a> Writer<'a> {
    pub(crate) fn new(dialect: &'a Dialect) -> Self {
        Self {
            dialect,
            sql: String::new(),
            params: Vec::new(),
        }
    }

    pub(crate) fn finish(self) -> Statement {
        Statement {
            sql: self.sql,
            params: self.params,
        }
    }

    pub(crate) fn push(&mut self, text: &str) {
        self.sql.push_str(text);
    }

    pub(crate) fn bind(&mut self, value: Value) {
        self.sql.push_str(self.dialect.placeholder);
        self.params.push(value);
    }

    /// Append another statement's text and values.
    pub(crate) fn append(&mut self, statement: Statement) {
        self.sql.push_str(&statement.sql);
        self.params.extend(statement.params);
    }

    /// Render into a separate writer. Used to drop an empty `And` or `Or`.
    fn sub(&self, condition: &Condition, columns: &Columns<'_>) -> Result<Statement, Error> {
        let mut inner = Writer::new(self.dialect);
        inner.condition(condition, columns)?;
        Ok(inner.finish())
    }

    pub(crate) fn condition(&mut self, condition: &Condition, columns: &Columns<'_>) -> Result<(), Error> {
        match condition {
            Condition::And(list) => self.join(list, "AND", columns),
            Condition::Or(list) => self.join(list, "OR", columns),
            Condition::Not(inner) => {
                let rendered = self.sub(inner, columns)?;
                match rendered.sql.as_str() {
                    // Nothing rendered is true, so its negation is false.
                    "" | "1 = 1" => self.push("1 = 0"),
                    "1 = 0" => self.push("1 = 1"),
                    _ => {
                        self.push("NOT (");
                        self.append(rendered);
                        self.push(")");
                    }
                }
                Ok(())
            }
            Condition::IgnoreCase(inner) => match inner.as_ref() {
                Condition::CompareFields(left, op, right) => {
                    let left = columns(left)?;
                    let right = columns(right)?;
                    self.upper_if(&left, left.upper_applies());
                    self.push(" ");
                    self.push(op.sql());
                    self.push(" ");
                    self.upper_if(&right, right.upper_applies());
                    Ok(())
                }
                Condition::And(_)
                | Condition::Or(_)
                | Condition::Not(_)
                | Condition::IgnoreCase(_)
                | Condition::Where(_) => Err(Error::new("ignore-case applies to one comparison")),
                other => self.compare(other, columns, true),
            },
            Condition::CompareFields(left, op, right) => {
                let left = columns(left)?;
                let right = columns(right)?;
                self.push(&left.sql);
                self.push(" ");
                self.push(op.sql());
                self.push(" ");
                self.push(&right.sql);
                Ok(())
            }
            Condition::Where(sql) => {
                // Wrapped, where Java appends the fragment as it is, so an `OR` in it cannot bind across the conditions
                // around it.
                self.push("(");
                self.push(sql);
                self.push(")");
                Ok(())
            }
            Condition::Matches(field, value) => {
                let column = columns(field)?;
                self.matches(&column, value.clone())
            }
            other => self.compare(other, columns, false),
        }
    }

    fn upper_if(&mut self, column: &Column, upper: bool) {
        if upper {
            self.push("UPPER(");
            self.push(&column.sql);
            self.push(")");
        } else {
            self.push(&column.sql);
        }
    }

    /// One field against values. `ignore_case` upper-cases the column when it is text, and
    /// every text value, as Java does.
    fn compare(&mut self, condition: &Condition, columns: &Columns<'_>, ignore_case: bool) -> Result<(), Error> {
        let fold = |value: &Value| -> Value {
            match value {
                Value::String(text) if ignore_case => Value::String(text.to_uppercase()),
                other => other.clone(),
            }
        };
        let (field, operator, values, null_form): (&FieldRef, &str, Vec<Value>, Option<&str>) = match condition {
            Condition::Equals(field, value) => (field, "=", vec![fold(value)], Some(" IS NULL")),
            Condition::NotEquals(field, value) => (field, "<>", vec![fold(value)], Some(" IS NOT NULL")),
            Condition::Like(field, value) => (field, "LIKE", vec![fold(value)], Some(" IS NULL")),
            Condition::NotLike(field, value) => (field, "NOT LIKE", vec![fold(value)], Some(" IS NOT NULL")),
            Condition::Less(field, value) => (field, "<", vec![fold(value)], None),
            Condition::Greater(field, value) => (field, ">", vec![fold(value)], None),
            Condition::LessEqual(field, value) => (field, "<=", vec![fold(value)], None),
            Condition::GreaterEqual(field, value) => (field, ">=", vec![fold(value)], None),
            Condition::Between(field, low, high) => (field, "BETWEEN", vec![fold(low), fold(high)], Some(" IS NULL")),
            Condition::NotBetween(field, low, high) => {
                (field, "NOT BETWEEN", vec![fold(low), fold(high)], Some(" IS NOT NULL"))
            }
            Condition::In(field, list) => (field, "IN", list.iter().map(fold).collect(), None),
            Condition::NotIn(field, list) => (field, "NOT IN", list.iter().map(fold).collect(), None),
            Condition::IsNull(field) => (field, " IS NULL", Vec::new(), None),
            Condition::IsNotNull(field) => (field, " IS NOT NULL", Vec::new(), None),
            Condition::Bound(field, op) => (field, op.sql(), vec![Value::Null], None),
            _ => return Err(Error::new("ignore-case applies to one comparison")),
        };
        if matches!(condition, Condition::In(..) | Condition::NotIn(..)) && values.is_empty() {
            self.push(if operator == "IN" { "1 = 0" } else { "1 = 1" });
            return Ok(());
        }
        let column = columns(field)?;
        let values: Vec<Value> = values.into_iter().map(|value| column.typed(value)).collect();
        let upper = ignore_case && column.upper_applies();
        if matches!(condition, Condition::IsNull(_) | Condition::IsNotNull(_)) {
            self.push(&column.sql);
            self.push(operator);
            return Ok(());
        }
        if let Some(null_form) = null_form {
            if values.iter().all(Value::is_null) {
                self.push(&column.sql);
                self.push(null_form);
                return Ok(());
            }
        }
        self.upper_if(&column, upper);
        self.push(" ");
        self.push(operator);
        self.push(" ");
        let placeholder = |out: &mut Self, value: Value| {
            if upper {
                out.push("UPPER(");
                out.bind(value);
                out.push(")");
            } else {
                out.bind(value);
            }
        };
        match condition {
            Condition::Between(..) | Condition::NotBetween(..) => {
                let mut values = values.into_iter();
                placeholder(self, values.next().unwrap_or(Value::Null));
                self.push(" AND ");
                placeholder(self, values.next().unwrap_or(Value::Null));
            }
            Condition::In(..) | Condition::NotIn(..) => {
                self.push("(");
                for (index, value) in values.into_iter().enumerate() {
                    if index > 0 {
                        self.push(", ");
                    }
                    placeholder(self, value);
                }
                self.push(")");
            }
            Condition::Bound(..) => {
                self.sql.push_str(self.dialect.placeholder);
                self.params.push(Value::Null);
            }
            _ => {
                for value in values {
                    placeholder(self, value);
                }
            }
        }
        Ok(())
    }

    /// An `Or` of no parts is false.
    fn join(&mut self, conditions: &[Condition], operator: &str, columns: &Columns<'_>) -> Result<(), Error> {
        if operator == "OR" && conditions.is_empty() {
            self.push("1 = 0");
            return Ok(());
        }
        let mut parts = Vec::new();
        for condition in conditions {
            let rendered = self.sub(condition, columns)?;
            // A part that renders as nothing is dropped, from an `Or` too, as Java `ListCondition` skips an
            // empty child: it does not make the `Or` true.
            if !rendered.sql.is_empty() {
                parts.push(rendered);
            }
        }
        if parts.is_empty() {
            return Ok(());
        }
        if parts.len() == 1 {
            self.append(parts.remove(0));
            return Ok(());
        }
        self.push("(");
        for (index, part) in parts.into_iter().enumerate() {
            if index > 0 {
                self.push(" ");
                self.push(operator);
                self.push(" ");
            }
            self.append(part);
        }
        self.push(")");
        Ok(())
    }

    fn matches(&mut self, column: &Column, value: Value) -> Result<(), Error> {
        match self.dialect.fts_style.as_deref() {
            Some("postgres-tsvector") => {
                let config = fts_config(self.dialect)?;
                // A `text-fts` field searches its generated column, which has the GIN index, as Java's `FtsSql`. Any
                // other column (a view alias, an expression) has none, so the vector is computed.
                match &column.fts {
                    Some(tsv) => self.push(&format!("{tsv} @@ plainto_tsquery('{config}', ")),
                    None => self.push(&format!(
                        "to_tsvector('{config}', {}) @@ plainto_tsquery('{config}', ",
                        column.sql
                    )),
                }
                self.bind(value);
                self.push(")");
                Ok(())
            }
            Some("mysql-fulltext") => {
                self.push(&format!("MATCH ({}) AGAINST (", column.sql));
                self.bind(value);
                self.push(" IN NATURAL LANGUAGE MODE)");
                Ok(())
            }
            Some(style) => Err(Error::new(format!("matches does not support fts-style {style}"))),
            None => Err(Error::new(
                "matches needs fts-style mysql-fulltext or postgres-tsvector",
            )),
        }
    }

    /// `LIMIT` and `OFFSET` per `offset-style`, with the values bound.
    pub(crate) fn page(&mut self, page: LimitOffset) {
        let limit_style = self.dialect.offset_style.as_deref() == Some("limit");
        match (limit_style, page.limit, page.offset) {
            (false, Some(limit), offset) => {
                self.push(" LIMIT ");
                self.bind(Value::I64(limit));
                self.push(" OFFSET ");
                self.bind(Value::I64(offset.unwrap_or(0)));
            }
            (false, None, Some(offset)) => {
                self.push(" OFFSET ");
                self.bind(Value::I64(offset));
            }
            (true, Some(limit), None) => {
                self.push(" LIMIT ");
                self.bind(Value::I64(limit));
            }
            (true, limit, Some(offset)) => {
                self.push(" LIMIT ");
                self.bind(Value::I64(offset));
                self.push(", ");
                self.bind(Value::I64(limit.unwrap_or(i64::MAX)));
            }
            (_, None, None) => {}
        }
    }

    /// `ORDER BY`, with `NULLS LAST` unless the entry says otherwise or the database sets
    /// `never-nulls`. With paging and no order, `ORDER BY 1`, as Java does.
    pub(crate) fn order_by(&mut self, order_by: &[OrderBy], paged: bool, columns: &Columns<'_>) -> Result<(), Error> {
        if order_by.is_empty() {
            if paged {
                self.push(" ORDER BY 1");
            }
            return Ok(());
        }
        self.push(" ORDER BY ");
        let write_nulls = !self.dialect.never_nulls;
        for (index, entry) in order_by.iter().enumerate() {
            if index > 0 {
                self.push(", ");
            }
            let column = columns(&entry.field)?;
            match entry.case {
                Some(upper) if column.text != Some(false) => {
                    self.push(if upper { "UPPER(" } else { "LOWER(" });
                    self.push(&column.sql);
                    self.push(")");
                }
                _ => self.push(&column.sql),
            }
            self.push(if entry.descending { " DESC" } else { " ASC" });
            if write_nulls {
                self.push(match entry.nulls {
                    Some(Nulls::First) => " NULLS FIRST",
                    Some(Nulls::Last) | None => " NULLS LAST",
                });
            }
        }
        Ok(())
    }
}

pub(crate) fn fts_config(dialect: &Dialect) -> Result<String, Error> {
    let config = dialect.fts_config.as_deref().unwrap_or("english");
    if !is_fts_config(config) {
        return Err(Error::new(format!("fts-config {config} is not an identifier")));
    }
    Ok(config.to_string())
}

fn is_fts_config(config: &str) -> bool {
    let mut chars = config.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

pub(crate) fn write_field(out: &mut String, dialect: &Dialect, field: &FieldRef) {
    if let Some(expr) = &field.expr {
        out.push_str(expr);
        return;
    }
    if let Some(alias) = &field.entity_alias {
        out.push_str(&quote_ident(dialect, alias));
        out.push('.');
    }
    out.push_str(&quote_ident(dialect, &field.name));
}

#[cfg(test)]
mod typed_tests {
    use super::*;

    fn column(java_type: &str) -> Column {
        Column {
            sql: "\"f\"".to_string(),
            text: Some(false),
            java_type: Some(java_type.to_string()),
            fts: None,
        }
    }

    #[test]
    fn text_is_converted_to_a_date_time_or_number_field() {
        let date = column("java.sql.Date").typed(Value::string("1791936000000"));
        assert!(matches!(date, Value::Date(_)), "{date:?}");
        assert!(matches!(
            column("java.sql.Date").typed(Value::string("2026-10-14")),
            Value::Date(_)
        ));
        assert!(matches!(
            column("java.sql.Timestamp").typed(Value::string("1791936000000")),
            Value::Timestamp(_)
        ));
        assert!(matches!(
            column("java.math.BigDecimal").typed(Value::string("12.50")),
            Value::Decimal(_)
        ));
        assert_eq!(
            column("java.lang.String").typed(Value::string("1791936000000")),
            Value::string("1791936000000")
        );
        assert_eq!(
            column("java.sql.Date").typed(Value::string("not a date")),
            Value::string("not a date")
        );
    }
}
