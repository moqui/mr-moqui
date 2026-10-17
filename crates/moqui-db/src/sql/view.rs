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

//! View-entity select and count, after `EntityFindBuilder`.
//!
//! Field names in conditions and order are alias names, or a member field through
//! [`FieldRef::aliased`]. The find selects the aliases in `options.fields`, or every alias.
//!
//! Only the members those aliases, the conditions, and the order read are joined, plus the
//! members on the join path back to the first member. The view's own `entity-condition` adds to
//! the where, having, order, and distinct. When the view has an aggregate alias, every selected
//! alias that is not an aggregate goes in `GROUP BY`.
//!
//! A member that is itself a view is a sub-select of that view's aliases. A `sub-select` member is
//! a sub-select of the fields the outer query reads, joined `LATERAL` when the database's
//! `from-lateral-style` is `lateral`, otherwise on its join fields. On a plain entity member,
//! function and complex aliases that read only that member are computed inside it, and a
//! condition on an alias that reads that member directly is applied inside it, as Java does.
//!
//! `${}` in a complex-alias or case expression is expanded per find. A reference written as
//! `ALIAS.UPPER_SNAKE_NAME`, Java's column naming, is rewritten to that member's column.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};

use moqui_lib::error::Error;

use crate::dialect::Dialect;
use crate::model::{
    is_text_java_type, AliasExpr, CaseAlias, ComplexAlias, ComplexField, ComplexPart, EntityDefinition, EntityModel,
    FieldAlias, MemberEntity, SubSelect,
};

use super::condition::{Column, Condition, FieldRef, LimitOffset, OrderBy, Writer};
use super::ident::{quote_column, quote_ident, quote_physical_ident};
use super::statement::{for_update, SelectOptions};
use super::{ConditionValues, Statement};

/// View-entity select. See the module notes.
pub fn select_view(
    view: &EntityDefinition,
    model: &EntityModel,
    dialect: &Dialect,
    values: &dyn ConditionValues,
    condition: Option<&Condition>,
    order_by: &[OrderBy],
    page: LimitOffset,
) -> Result<Statement, Error> {
    select_view_with(
        view,
        model,
        dialect,
        values,
        condition,
        order_by,
        page,
        &SelectOptions::default(),
    )
}

/// [`select_view`] with [`SelectOptions`].
#[expect(clippy::too_many_arguments)]
pub fn select_view_with(
    view: &EntityDefinition,
    model: &EntityModel,
    dialect: &Dialect,
    values: &dyn ConditionValues,
    condition: Option<&Condition>,
    order_by: &[OrderBy],
    page: LimitOffset,
    options: &SelectOptions,
) -> Result<Statement, Error> {
    let renderer = Renderer::new(view, model, dialect, values)?;
    let query = renderer.query(condition, options, order_by)?;
    if options.for_update && (query.distinct || renderer.grouped()) {
        return Err(Error::new("a select cannot lock rows with distinct or an aggregate"));
    }
    let mut out = Writer::new(dialect);
    renderer.write_select(&mut out, &query, true)?;
    let columns = |field: &FieldRef| renderer.column(field, true);
    out.order_by(&query.order_by, page.is_set(), &columns)?;
    out.page(page);
    if options.for_update {
        for_update(&mut out);
    }
    Ok(out.finish())
}

/// View-entity count. With a `GROUP BY` or a distinct select, the count is over a sub-select,
/// as Java `makeCountFunction` does.
pub fn count_view(
    view: &EntityDefinition,
    model: &EntityModel,
    dialect: &Dialect,
    values: &dyn ConditionValues,
    condition: Option<&Condition>,
    options: &SelectOptions,
) -> Result<Statement, Error> {
    let renderer = Renderer::new(view, model, dialect, values)?;
    let query = renderer.query(condition, options, &[])?;
    let mut out = Writer::new(dialect);
    if renderer.grouped() || query.distinct {
        out.push("SELECT COUNT(*) FROM (");
        renderer.write_select(&mut out, &query, true)?;
        out.push(") TEMP_NAME");
    } else {
        renderer.write_select(&mut out, &query, false)?;
    }
    Ok(out.finish())
}

/// One find against the view, with the view's own `entity-condition` merged in.
struct Query<'a> {
    selected: Vec<&'a FieldAlias>,
    where_condition: Option<Condition>,
    having: Option<Condition>,
    order_by: Vec<OrderBy>,
    distinct: bool,
}

/// A column a sub-select member shows to the outer query.
#[derive(Debug, Clone, PartialEq)]
enum Exposure {
    /// A member field, shown under its field name.
    Field(String),
    /// An alias computed inside the sub-select, shown under the alias name.
    Computed { name: String, sql: String, aggregate: bool },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Table,
    /// A view member: a sub-select of all its aliases.
    View,
    SubSelect {
        lateral: bool,
    },
}

struct Renderer<'a> {
    view: &'a EntityDefinition,
    model: &'a EntityModel,
    dialect: &'a Dialect,
    values: &'a dyn ConditionValues,
    exposures: RefCell<BTreeMap<String, Vec<Exposure>>>,
}

impl<'a> Renderer<'a> {
    fn new(
        view: &'a EntityDefinition,
        model: &'a EntityModel,
        dialect: &'a Dialect,
        values: &'a dyn ConditionValues,
    ) -> Result<Self, Error> {
        if !view.is_view {
            return Err(Error::new(format!("{} is not a view-entity", view.full_name)));
        }
        if view.member_entities.is_empty() {
            return Err(Error::new(format!(
                "view-entity {} has no member-entity",
                view.full_name
            )));
        }
        Ok(Self {
            view,
            model,
            dialect,
            values,
            exposures: RefCell::new(BTreeMap::new()),
        })
    }

    fn query(
        &self,
        condition: Option<&Condition>,
        options: &SelectOptions,
        order_by: &[OrderBy],
    ) -> Result<Query<'a>, Error> {
        let own = self.view.entity_condition.as_ref();
        let distinct = options.distinct || own.is_some_and(|own| own.distinct);
        let mut names: Vec<&str> = options.fields.iter().map(String::as_str).collect();
        if distinct && !names.is_empty() {
            // A distinct select can only be ordered by what it selects, so the order-by fields are selected too, as
            // Java's EntityFindBase adds them.
            for order in order_by {
                if self.view.alias(&order.field.name).is_some() && !names.contains(&order.field.name.as_str()) {
                    names.push(order.field.name.as_str());
                }
            }
        }
        let selected: Vec<&FieldAlias> = if names.is_empty() {
            self.view.aliases.iter().collect()
        } else {
            names
                .iter()
                .map(|name| {
                    self.view
                        .alias(name)
                        .ok_or_else(|| Error::new(format!("{} has no field {name}", self.view.full_name)))
                })
                .collect::<Result<_, _>>()?
        };
        if selected.is_empty() {
            return Err(Error::new(format!(
                "view-entity {} has no alias to select",
                self.view.full_name
            )));
        }
        let own_where = match own {
            Some(own) => own.conditions.build(self.view, self.model, None, self.values)?,
            None => None,
        };
        let own_having = match own.and_then(|own| own.having.as_ref()) {
            Some(having) => having.build(self.view, self.model, None, self.values)?,
            None => None,
        };
        let where_condition = Condition::and_opt(condition.cloned(), own_where);
        let having = Condition::and_opt(options.having.clone(), own_having);
        if having.is_some() && !self.grouped() {
            return Err(Error::new(format!(
                "view {} has no aggregate alias, so a having condition has nothing to apply to",
                self.view.full_name
            )));
        }
        if let Some(found) = &where_condition {
            if found.any_field(&mut |field| self.reads_aggregate(field)) {
                return Err(Error::new(
                    "a condition on an aggregate alias must be a having condition, not a where condition",
                ));
            }
        }
        let mut ordering: Vec<OrderBy> = order_by.to_vec();
        if let Some(own) = own {
            ordering.extend(own.order_by.iter().filter_map(|text| OrderBy::parse(text)));
        }
        Ok(Query {
            selected,
            where_condition,
            having,
            order_by: ordering,
            distinct,
        })
    }

    fn member(&self, entity_alias: &str) -> Result<&'a MemberEntity, Error> {
        self.view.member(entity_alias).ok_or_else(|| {
            Error::new(format!(
                "could not find member-entity with entity-alias {entity_alias} in view-entity {}",
                self.view.full_name
            ))
        })
    }

    fn member_entity(&self, member: &MemberEntity) -> Result<&'a EntityDefinition, Error> {
        self.model.entity(&member.entity_name)
    }

    fn kind(&self, member: &MemberEntity) -> Result<Kind, Error> {
        if member.sub_select.is_on() {
            let lateral =
                member.sub_select == SubSelect::On && self.dialect.from_lateral_style.as_deref() == Some("lateral");
            return Ok(Kind::SubSelect { lateral });
        }
        Ok(if self.member_entity(member)?.is_view {
            Kind::View
        } else {
            Kind::Table
        })
    }

    /// True when any alias aggregates outside a sub-select: Java `hasFunctionAlias`.
    fn grouped(&self) -> bool {
        self.view
            .aliases
            .iter()
            .any(|alias| alias.aggregates() && self.computed_inside(alias).is_none())
    }

    /// The plain sub-select member that computes this alias inside, when there is one.
    fn computed_inside(&self, alias: &FieldAlias) -> Option<&'a MemberEntity> {
        if alias.function.is_none() && alias.expr == AliasExpr::Column {
            return None;
        }
        let used = alias.entity_aliases_used();
        if used.len() != 1 {
            return None;
        }
        let member = self.view.member(used[0])?;
        if !member.sub_select.is_on() {
            return None;
        }
        let entity = self.model.entity(&member.entity_name).ok()?;
        (!entity.is_view).then_some(member)
    }

    fn reads_aggregate(&self, field: &FieldRef) -> bool {
        if field.entity_alias.is_some() || field.expr.is_some() {
            return false;
        }
        self.view
            .alias(&field.name)
            .is_some_and(|alias| alias.aggregates() && self.computed_inside(alias).is_none())
    }

    /// A member field as the outer query reads it.
    fn member_column(&self, entity_alias: &str, field: &str) -> Result<String, Error> {
        let member = self.member(entity_alias)?;
        let entity = self.member_entity(member)?;
        let qualifier = quote_ident(self.dialect, entity_alias);
        match self.kind(member)? {
            Kind::Table => {
                let definition = entity
                    .field(field)
                    .ok_or_else(|| Error::new(format!("invalid field name {field} for entity {}", entity.full_name)))?;
                Ok(format!(
                    "{qualifier}.{}",
                    quote_column(self.dialect, &definition.column_name)
                ))
            }
            Kind::View => {
                if entity.alias(field).is_none() {
                    return Err(Error::new(format!(
                        "invalid field name {field} for entity {}",
                        entity.full_name
                    )));
                }
                Ok(format!("{qualifier}.{}", quote_ident(self.dialect, field)))
            }
            Kind::SubSelect { .. } => {
                if !entity.has_field(field) {
                    return Err(Error::new(format!(
                        "invalid field name {field} for entity {}",
                        entity.full_name
                    )));
                }
                self.expose(entity_alias, Exposure::Field(field.to_string()));
                Ok(format!("{qualifier}.{}", quote_ident(self.dialect, field)))
            }
        }
    }

    fn expose(&self, entity_alias: &str, exposure: Exposure) {
        let mut exposures = self.exposures.borrow_mut();
        let list = exposures.entry(entity_alias.to_string()).or_default();
        if !list.contains(&exposure) {
            list.push(exposure);
        }
    }

    /// An alias's SQL and whether it aggregates in the outer query.
    fn alias_sql(&self, alias: &FieldAlias) -> Result<(String, bool), Error> {
        if let Some(member) = self.computed_inside(alias) {
            let entity = self.member_entity(member)?;
            let inner_column = |entity_alias: &str, field: &str| -> Result<String, Error> {
                let definition = entity
                    .field(field)
                    .ok_or_else(|| Error::new(format!("invalid field name {field} for entity {}", entity.full_name)))?;
                Ok(format!(
                    "{}.{}",
                    quote_ident(self.dialect, entity_alias),
                    quote_column(self.dialect, &definition.column_name)
                ))
            };
            let sql = self.alias_expression(alias, &inner_column)?;
            self.expose(
                &member.entity_alias,
                Exposure::Computed {
                    name: alias.name.clone(),
                    sql,
                    aggregate: alias.aggregates(),
                },
            );
            let outer = format!(
                "{}.{}",
                quote_ident(self.dialect, &member.entity_alias),
                quote_ident(self.dialect, &alias.name)
            );
            return Ok((outer, false));
        }
        let column = |entity_alias: &str, field: &str| self.member_column(entity_alias, field);
        Ok((self.alias_expression(alias, &column)?, alias.aggregates()))
    }

    /// The alias expression, with member columns written by `column`.
    fn alias_expression(
        &self,
        alias: &FieldAlias,
        column: &dyn Fn(&str, &str) -> Result<String, Error>,
    ) -> Result<String, Error> {
        let body = match &alias.expr {
            AliasExpr::Column => {
                let entity_alias = alias
                    .entity_alias
                    .as_deref()
                    .ok_or_else(|| Error::new(format!("alias {} has no entity-alias", alias.name)))?;
                column(entity_alias, alias.field_name())?
            }
            AliasExpr::Complex(complex) => {
                let mut sql = String::new();
                self.complex(&mut sql, complex, alias.function.is_none(), column)?;
                sql
            }
            AliasExpr::Case(case) => self.case(case, column)?,
        };
        match alias.function.as_deref() {
            Some(function) => Ok(format!("{}{body})", function_prefix(function)?)),
            None => Ok(body),
        }
    }

    /// Java `buildComplexAliasName`.
    fn complex(
        &self,
        sql: &mut String,
        complex: &ComplexAlias,
        add_parens: bool,
        column: &dyn Fn(&str, &str) -> Result<String, Error>,
    ) -> Result<(), Error> {
        if let Some(expression) = &complex.expression {
            sql.push_str(&self.raw_sql(expression, column)?);
        }
        if complex.parts.is_empty() {
            return Ok(());
        }
        if let Some(function) = &complex.function {
            check_word(function)?;
            sql.push_str(function);
            sql.push('(');
            for (index, part) in complex.parts.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                self.complex_part(sql, part, column)?;
            }
            sql.push(')');
            return Ok(());
        }
        let operator = complex.operator.as_deref().unwrap_or("+");
        check_operator(operator)?;
        let parens = add_parens && complex.parts.len() > 1;
        if parens {
            sql.push('(');
        }
        for (index, part) in complex.parts.iter().enumerate() {
            if index > 0 {
                sql.push(' ');
                sql.push_str(operator);
                sql.push(' ');
            }
            self.complex_part(sql, part, column)?;
        }
        if parens {
            sql.push(')');
        }
        Ok(())
    }

    fn complex_part(
        &self,
        sql: &mut String,
        part: &ComplexPart,
        column: &dyn Fn(&str, &str) -> Result<String, Error>,
    ) -> Result<(), Error> {
        match part {
            ComplexPart::Alias(inner) => self.complex(sql, inner, true, column),
            ComplexPart::Field(field) => self.complex_field(sql, field, column),
        }
    }

    fn complex_field(
        &self,
        sql: &mut String,
        field: &ComplexField,
        column: &dyn Fn(&str, &str) -> Result<String, Error>,
    ) -> Result<(), Error> {
        let name = column(&field.entity_alias, &field.field)?;
        if let Some(function) = &field.function {
            sql.push_str(&function_prefix(function)?);
        }
        match &field.default_value {
            Some(default_value) => {
                sql.push_str("COALESCE(");
                sql.push_str(&name);
                sql.push(',');
                sql.push_str(&self.raw_sql(default_value, column)?);
                sql.push(')');
            }
            None => sql.push_str(&name),
        }
        if field.function.is_some() {
            sql.push(')');
        }
        Ok(())
    }

    fn case(&self, case: &CaseAlias, column: &dyn Fn(&str, &str) -> Result<String, Error>) -> Result<String, Error> {
        let mut sql = String::from("CASE");
        if let Some(expression) = &case.expression {
            sql.push(' ');
            sql.push_str(&self.raw_sql(expression, column)?);
        }
        for (when, then) in &case.whens {
            sql.push_str(" WHEN ");
            sql.push_str(&self.raw_sql(when, column)?);
            sql.push_str(" THEN ");
            self.complex(&mut sql, then, true, column)?;
        }
        if let Some(otherwise) = &case.otherwise {
            sql.push_str(" ELSE ");
            self.complex(&mut sql, otherwise, true, column)?;
        }
        sql.push_str(" END");
        Ok(sql)
    }

    /// SQL text from the definition: `${}` expanded, then `ALIAS.UPPER_SNAKE` member references
    /// rewritten to columns. Text in single quotes is left alone.
    fn raw_sql(&self, text: &str, column: &dyn Fn(&str, &str) -> Result<String, Error>) -> Result<String, Error> {
        let text = if text.contains("${") {
            self.values.expand(text)?
        } else {
            text.to_string()
        };
        let mut out = String::with_capacity(text.len());
        let chars: Vec<char> = text.chars().collect();
        let mut index = 0;
        let mut quoted = false;
        while index < chars.len() {
            let ch = chars[index];
            if ch == '\'' {
                quoted = !quoted;
                out.push(ch);
                index += 1;
                continue;
            }
            let starts_word = !quoted
                && (ch.is_ascii_alphabetic() || ch == '_')
                && (index == 0 || !is_word(chars[index - 1]) && chars[index - 1] != '.' && chars[index - 1] != '"');
            if !starts_word {
                out.push(ch);
                index += 1;
                continue;
            }
            let start = index;
            while index < chars.len() && is_word(chars[index]) {
                index += 1;
            }
            let first: String = chars[start..index].iter().collect();
            if index + 1 < chars.len()
                && chars[index] == '.'
                && (chars[index + 1].is_ascii_alphabetic() || chars[index + 1] == '_')
            {
                let second_start = index + 1;
                let mut end = second_start;
                while end < chars.len() && is_word(chars[end]) {
                    end += 1;
                }
                let second: String = chars[second_start..end].iter().collect();
                if let Some(field) = self.java_column_field(&first, &second)? {
                    out.push_str(&column(&first, &field)?);
                    index = end;
                    continue;
                }
            }
            out.push_str(&first);
        }
        Ok(out)
    }

    /// The member field a Java column reference `ALIAS.COLUMN_NAME` names, if any.
    fn java_column_field(&self, entity_alias: &str, column: &str) -> Result<Option<String>, Error> {
        let Some(member) = self.view.member(entity_alias) else {
            return Ok(None);
        };
        let entity = self.member_entity(member)?;
        for name in entity.field_names() {
            if name == column || camel_case_to_underscored(name).eq_ignore_ascii_case(column) {
                return Ok(Some(name.to_string()));
            }
        }
        Ok(None)
    }

    /// A field in a condition or order: an alias name, or a member field when aliased.
    fn column(&self, field: &FieldRef, allow_aggregate: bool) -> Result<Column, Error> {
        if field.expr.is_some() {
            return Ok(Column::plain(self.dialect, field));
        }
        if let Some(entity_alias) = &field.entity_alias {
            let member = self.member(entity_alias)?;
            let entity = self.member_entity(member)?;
            return Ok(Column {
                sql: self.member_column(entity_alias, &field.name)?,
                text: entity.java_type_of(&field.name).map(is_text_java_type),
                java_type: entity.java_type_of(&field.name).map(str::to_string),
                fts: None,
            });
        }
        let alias = self
            .view
            .alias(&field.name)
            .ok_or_else(|| Error::new(format!("{} has no field {}", self.view.full_name, field.name)))?;
        let (sql, aggregate) = self.alias_sql(alias)?;
        if aggregate && !allow_aggregate {
            return Err(Error::new(format!(
                "a condition on aggregate alias {} must be a having condition",
                alias.name
            )));
        }
        Ok(Column {
            sql,
            text: (!alias.java_type.is_empty()).then(|| is_text_java_type(&alias.java_type)),
            java_type: (!alias.java_type.is_empty()).then(|| alias.java_type.clone()),
            fts: None,
        })
    }

    /// The sub-select member a field reads directly, if any.
    fn sub_select_member(&self, field: &FieldRef) -> Option<(&'a MemberEntity, String)> {
        if field.expr.is_some() {
            return None;
        }
        let (entity_alias, member_field) = match &field.entity_alias {
            Some(entity_alias) => (entity_alias.as_str(), field.name.clone()),
            None => {
                let alias = self.view.alias(&field.name)?;
                if self.computed_inside(alias).is_some() {
                    return None;
                }
                (alias.direct_member()?, alias.field_name().to_string())
            }
        };
        let member = self.view.member(entity_alias)?;
        member.sub_select.is_on().then_some((member, member_field))
    }

    /// The where condition without the parts that move into sub-selects. An `or` stays whole
    /// unless every part moves, as Java `ListCondition.filter` does.
    fn outer_part(&self, condition: &Condition) -> Option<Condition> {
        match condition {
            Condition::And(list) => {
                let kept: Vec<Condition> = list.iter().filter_map(|child| self.outer_part(child)).collect();
                match kept.len() {
                    0 => None,
                    1 => kept.into_iter().next(),
                    _ => Some(Condition::And(kept)),
                }
            }
            Condition::Or(list) => list
                .iter()
                .any(|child| self.outer_part(child).is_some())
                .then(|| condition.clone()),
            Condition::Not(inner) => self.outer_part(inner).map(|_| condition.clone()),
            Condition::IgnoreCase(inner) => self.outer_part(inner).map(|_| condition.clone()),
            Condition::CompareFields(left, _, right) => {
                match (self.sub_select_member(left), self.sub_select_member(right)) {
                    (Some((a, _)), Some((b, _))) if a.entity_alias == b.entity_alias => None,
                    _ => Some(condition.clone()),
                }
            }
            leaf => {
                let mut moved = false;
                leaf.visit_fields(&mut |field| moved = self.sub_select_member(field).is_some());
                (!moved).then(|| leaf.clone())
            }
        }
    }

    /// The parts of the where condition that apply inside `member`'s sub-select, with fields
    /// renamed to the member's own field names.
    fn inner_part(&self, condition: &Condition, member: &MemberEntity) -> Option<Condition> {
        let on_member = |field: &FieldRef| {
            self.sub_select_member(field)
                .is_some_and(|(found, _)| found.entity_alias == member.entity_alias)
        };
        let rename = |condition: Condition| {
            condition.map_fields(&mut |field| match self.sub_select_member(&field) {
                Some((_, member_field)) => FieldRef::new(member_field),
                None => field,
            })
        };
        match condition {
            Condition::And(list) => {
                let kept: Vec<Condition> = list.iter().filter_map(|child| self.inner_part(child, member)).collect();
                match kept.len() {
                    0 => None,
                    1 => kept.into_iter().next(),
                    _ => Some(Condition::And(kept)),
                }
            }
            Condition::Or(list) => list
                .iter()
                .all(|child| self.inner_part(child, member).is_some())
                .then(|| rename(condition.clone())),
            other => {
                let mut all = true;
                let mut any = false;
                other.visit_fields(&mut |field| {
                    if on_member(field) {
                        any = true;
                    } else {
                        all = false;
                    }
                });
                (any && all).then(|| rename(other.clone()))
            }
        }
    }

    /// Entity aliases the find reads, plus the join path back to the first member.
    fn used_members(&self, query: &Query<'_>) -> Result<HashSet<String>, Error> {
        let mut used: HashSet<String> = HashSet::new();
        let add_field = |field: &FieldRef, used: &mut HashSet<String>| {
            if field.expr.is_some() {
                return;
            }
            if let Some(entity_alias) = &field.entity_alias {
                used.insert(entity_alias.clone());
            } else if let Some(alias) = self.view.alias(&field.name) {
                used.extend(alias.entity_aliases_used().into_iter().map(str::to_string));
            }
        };
        for alias in &query.selected {
            used.extend(alias.entity_aliases_used().into_iter().map(str::to_string));
        }
        for condition in query.where_condition.iter().chain(query.having.iter()) {
            condition.visit_fields(&mut |field| add_field(field, &mut used));
        }
        for entry in &query.order_by {
            add_field(&entry.field, &mut used);
        }
        let main = self
            .view
            .member_entities
            .iter()
            .find(|member| member.join_from_alias.is_none())
            .map(|member| member.entity_alias.clone());
        let mut joined: HashSet<String> = main.iter().cloned().collect();
        let start: Vec<String> = used.iter().cloned().collect();
        for entity_alias in start {
            self.join_path(&entity_alias, &mut used, &mut joined, 0)?;
        }
        Ok(used)
    }

    /// Java `expandJoinFromAlias`.
    fn join_path(
        &self,
        entity_alias: &str,
        used: &mut HashSet<String>,
        joined: &mut HashSet<String>,
        depth: usize,
    ) -> Result<(), Error> {
        if joined.contains(entity_alias) {
            return Ok(());
        }
        if depth > self.view.member_entities.len() {
            return Err(Error::new(format!(
                "view {} member alias {entity_alias} joins from itself",
                self.view.full_name
            )));
        }
        let member = self.member(entity_alias)?;
        let join_from = member.join_from_alias.as_deref().ok_or_else(|| {
            Error::new(format!(
                "in view-entity {} the member-entity for entity-alias {entity_alias} has no join-from-alias and is not the first member-entity",
                self.view.full_name
            ))
        })?;
        self.join_path(join_from, used, joined, depth + 1)?;
        joined.insert(entity_alias.to_string());
        used.insert(entity_alias.to_string());
        used.insert(join_from.to_string());
        Ok(())
    }

    /// `SELECT [DISTINCT] list FROM ... WHERE ... GROUP BY ... HAVING ...`. With `list` false the
    /// select list is `COUNT(*)` and there is no group or having.
    fn write_select(&self, out: &mut Writer<'_>, query: &Query<'_>, list: bool) -> Result<(), Error> {
        // Resolve every column first, so the sub-selects know what to show.
        let mut items = Vec::new();
        let mut group_by = Vec::new();
        let grouped = list && self.grouped();
        for alias in &query.selected {
            let (sql, aggregate) = self.alias_sql(alias)?;
            if grouped && !aggregate {
                group_by.push(sql.clone());
            }
            let plain = format!(
                "{}.{}",
                quote_ident(self.dialect, alias.entity_alias.as_deref().unwrap_or("")),
                quote_ident(self.dialect, &alias.name)
            );
            if sql == plain {
                items.push(sql);
            } else {
                items.push(format!("{sql} AS {}", quote_ident(self.dialect, &alias.name)));
            }
        }
        let outer_where = query
            .where_condition
            .as_ref()
            .and_then(|condition| self.outer_part(condition));
        let resolve = |field: &FieldRef| self.column(field, false);
        let resolve_having = |field: &FieldRef| self.column(field, true);
        // A dry render resolves the condition and order columns; the text is written below.
        if let Some(condition) = &outer_where {
            Writer::new(self.dialect).condition(condition, &resolve)?;
        }
        if let Some(having) = &query.having {
            Writer::new(self.dialect).condition(having, &resolve_having)?;
        }
        for entry in &query.order_by {
            self.column(&entry.field, true)?;
        }
        let used = self.used_members(query)?;
        let from = self.write_from(&used, query.where_condition.as_ref())?;

        if list {
            out.push(if query.distinct { "SELECT DISTINCT " } else { "SELECT " });
            out.push(&items.join(", "));
        } else {
            out.push("SELECT COUNT(*)");
        }
        out.push(" FROM ");
        out.append(from);
        if let Some(condition) = &outer_where {
            let mut inner = Writer::new(self.dialect);
            inner.condition(condition, &resolve)?;
            let rendered = inner.finish();
            if !rendered.sql.is_empty() {
                out.push(" WHERE ");
                out.append(rendered);
            }
        }
        if grouped && !group_by.is_empty() {
            out.push(" GROUP BY ");
            out.push(&group_by.join(", "));
        }
        if list {
            if let Some(having) = &query.having {
                let mut inner = Writer::new(self.dialect);
                inner.condition(having, &resolve_having)?;
                let rendered = inner.finish();
                if !rendered.sql.is_empty() {
                    out.push(" HAVING ");
                    out.append(rendered);
                }
            }
        }
        Ok(())
    }

    /// Java `makeSqlFromClause` for a view: joined members in document order, then any used
    /// member that is not joined, after a comma.
    fn write_from(&self, used: &HashSet<String>, where_condition: Option<&Condition>) -> Result<Statement, Error> {
        let use_parens = self.dialect.join_style.as_deref() != Some("ansi-no-parenthesis");
        // ON conditions and join fields can add exposures, so resolve them before any sub-select.
        let mut joins = Vec::new();
        for member in &self.view.member_entities {
            let Some(join_from) = member.join_from_alias.as_deref() else {
                continue;
            };
            if !used.contains(&member.entity_alias) || !used.contains(join_from) {
                continue;
            }
            joins.push((member, join_from, self.join_on(member, join_from)?));
        }
        let mut out = Writer::new(self.dialect);
        let mut joined: HashSet<&str> = HashSet::new();
        let mut empty = true;
        for (index, (member, join_from, on)) in joins.into_iter().enumerate() {
            if index == 0 {
                if use_parens {
                    out.push("(");
                }
                let link = self.member(join_from)?;
                self.member_source(&mut out, link, where_condition)?;
                joined.insert(join_from);
            } else if !joined.contains(join_from) {
                return Err(Error::new(format!(
                    "tried to link the {} alias to the {join_from} alias of the {} view-entity, but it is not the first member-entity and has not been joined to a previous member-entity",
                    member.entity_alias, self.view.full_name
                )));
            }
            empty = false;
            joined.insert(&member.entity_alias);
            out.push(if member.join_optional {
                " LEFT OUTER JOIN "
            } else {
                " INNER JOIN "
            });
            self.member_source(&mut out, member, where_condition)?;
            out.push(" ON ");
            out.append(on);
        }
        if !empty && use_parens {
            out.push(")");
        }
        for member in &self.view.member_entities {
            if !used.contains(&member.entity_alias) || joined.contains(member.entity_alias.as_str()) {
                continue;
            }
            if !empty {
                out.push(", ");
            }
            empty = false;
            self.member_source(&mut out, member, where_condition)?;
        }
        Ok(out.finish())
    }

    /// The `ON` text for a joined member: its key map, then its own `entity-condition`. A lateral
    /// sub-select joins `ON 1=1`; its keys are inside it.
    fn join_on(&self, member: &MemberEntity, join_from: &str) -> Result<Statement, Error> {
        let mut out = Writer::new(self.dialect);
        if self.kind(member)? == (Kind::SubSelect { lateral: true }) {
            out.push("1=1");
            return Ok(out.finish());
        }
        let pairs = self.join_keys(member)?;
        for (index, (field, related)) in pairs.iter().enumerate() {
            if index > 0 {
                out.push(" AND ");
            }
            out.push(&self.member_column(join_from, field)?);
            out.push(" = ");
            out.push(&self.member_column(&member.entity_alias, related)?);
        }
        if let Some(condition) = &member.entity_condition {
            let built = condition
                .conditions
                .build(self.view, self.model, Some(member), self.values)?;
            if let Some(built) = built {
                let resolve = |field: &FieldRef| self.column(field, false);
                let mut inner = Writer::new(self.dialect);
                inner.condition(&built, &resolve)?;
                let rendered = inner.finish();
                if !rendered.sql.is_empty() {
                    if !pairs.is_empty() {
                        out.push(" AND ");
                    }
                    out.append(rendered);
                }
            }
        }
        if out.sql.is_empty() {
            out.push("1=1");
        }
        Ok(out.finish())
    }

    /// Key map pairs: join-from field to member field. A `related` that is not a member field
    /// falls back to the member's single primary key when there is one key map, as Java
    /// `appendJoinConditions` does.
    fn join_keys(&self, member: &MemberEntity) -> Result<Vec<(String, String)>, Error> {
        let entity = self.member_entity(member)?;
        let pks = entity.pk_field_names();
        Ok(member
            .key_map
            .iter()
            .map(|key| {
                let mut related = key
                    .related
                    .clone()
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| key.field_name.clone());
                if !entity.has_field(&related) && pks.len() == 1 && member.key_map.len() == 1 {
                    related = pks[0].to_string();
                }
                (key.field_name.clone(), related)
            })
            .collect())
    }

    /// The table, view sub-select, or member sub-select, and the member alias.
    fn member_source(
        &self,
        out: &mut Writer<'_>,
        member: &MemberEntity,
        where_condition: Option<&Condition>,
    ) -> Result<(), Error> {
        let entity = self.member_entity(member)?;
        match self.kind(member)? {
            Kind::Table => out.push(&quote_physical_ident(self.dialect, &entity.table_name)),
            Kind::View => {
                let nested = Renderer::new(entity, self.model, self.dialect, self.values)?;
                let query = nested.query(None, &SelectOptions::default(), &[])?;
                out.push("(");
                nested.write_select(out, &query, true)?;
                out.push(")");
            }
            Kind::SubSelect { lateral } => {
                if lateral {
                    out.push("LATERAL ");
                }
                out.push("(");
                self.sub_select(out, member, entity, lateral, where_condition)?;
                out.push(")");
            }
        }
        out.push(" ");
        out.push(&quote_ident(self.dialect, &member.entity_alias));
        Ok(())
    }

    /// Java `makeSqlMemberSubSelect`.
    fn sub_select(
        &self,
        out: &mut Writer<'_>,
        member: &MemberEntity,
        entity: &EntityDefinition,
        lateral: bool,
        where_condition: Option<&Condition>,
    ) -> Result<(), Error> {
        let mut exposures = self
            .exposures
            .borrow()
            .get(&member.entity_alias)
            .cloned()
            .unwrap_or_default();
        let pairs = self.join_keys(member)?;
        let join_from = member.join_from_alias.as_deref();
        if !lateral {
            for (_, related) in &pairs {
                let exposure = Exposure::Field(related.clone());
                if !exposures.contains(&exposure) {
                    exposures.push(exposure);
                }
            }
        }
        if exposures.is_empty() {
            return Err(Error::new(format!(
                "sub-select member {} of view-entity {} has nothing to select",
                member.entity_alias, self.view.full_name
            )));
        }
        let inner_condition = where_condition.and_then(|condition| self.inner_part(condition, member));
        let correlation: Vec<(String, String)> = match (lateral, join_from) {
            (true, Some(join_from)) => pairs
                .iter()
                .map(|(field, related)| Ok((self.member_column(join_from, field)?, related.clone())))
                .collect::<Result<_, Error>>()?,
            _ => Vec::new(),
        };
        if entity.is_view {
            let fields: Vec<String> = exposures
                .iter()
                .filter_map(|exposure| match exposure {
                    Exposure::Field(name) => Some(name.clone()),
                    Exposure::Computed { .. } => None,
                })
                .collect();
            let mut condition = inner_condition;
            for (outer, related) in correlation {
                let link = Condition::compare_fields(
                    FieldRef::expression("outer", outer),
                    super::CompareOp::Equals,
                    FieldRef::new(related),
                );
                condition = Condition::and_opt(condition, Some(link));
            }
            let nested = Renderer::new(entity, self.model, self.dialect, self.values)?;
            let options = SelectOptions {
                fields,
                ..SelectOptions::default()
            };
            let query = nested.query(condition.as_ref(), &options, &[])?;
            return nested.write_select(out, &query, true);
        }
        let qualifier = quote_ident(self.dialect, &member.entity_alias);
        let column = |field: &str| -> Result<String, Error> {
            let definition = entity
                .field(field)
                .ok_or_else(|| Error::new(format!("invalid field name {field} for entity {}", entity.full_name)))?;
            Ok(format!(
                "{qualifier}.{}",
                quote_column(self.dialect, &definition.column_name)
            ))
        };
        let mut items = Vec::new();
        let mut group_by = Vec::new();
        let mut aggregates = false;
        for exposure in &exposures {
            match exposure {
                Exposure::Field(name) => {
                    let sql = column(name)?;
                    group_by.push(sql.clone());
                    items.push(format!("{sql} AS {}", quote_ident(self.dialect, name)));
                }
                Exposure::Computed { name, sql, aggregate } => {
                    if *aggregate {
                        aggregates = true;
                    } else {
                        group_by.push(sql.clone());
                    }
                    items.push(format!("{sql} AS {}", quote_ident(self.dialect, name)));
                }
            }
        }
        out.push("SELECT ");
        out.push(&items.join(", "));
        out.push(" FROM ");
        out.push(&quote_physical_ident(self.dialect, &entity.table_name));
        out.push(" ");
        out.push(&qualifier);
        let mut wrote_where = false;
        for (index, (outer, related)) in correlation.iter().enumerate() {
            out.push(if index == 0 { " WHERE " } else { " AND " });
            out.push(outer);
            out.push(" = ");
            out.push(&column(related)?);
            wrote_where = true;
        }
        if let Some(condition) = &inner_condition {
            let resolve = |field: &FieldRef| -> Result<Column, Error> {
                if field.expr.is_some() {
                    return Ok(Column::plain(self.dialect, field));
                }
                Ok(Column {
                    sql: column(&field.name)?,
                    text: entity.field(&field.name).map(|definition| definition.is_text()),
                    java_type: entity.java_type_of(&field.name).map(str::to_string),
                    fts: None,
                })
            };
            let mut inner = Writer::new(self.dialect);
            inner.condition(condition, &resolve)?;
            let rendered = inner.finish();
            if !rendered.sql.is_empty() {
                out.push(if wrote_where { " AND " } else { " WHERE " });
                out.append(rendered);
            }
        }
        if aggregates && !group_by.is_empty() {
            out.push(" GROUP BY ");
            out.push(&group_by.join(", "));
        }
        Ok(())
    }
}

/// Java `getFunctionPrefix`: `COUNT(DISTINCT ` for `count-distinct`, else the upper-cased name.
fn function_prefix(function: &str) -> Result<String, Error> {
    if function == "count-distinct" {
        return Ok("COUNT(DISTINCT ".to_string());
    }
    check_word(function)?;
    Ok(format!("{}(", function.to_ascii_uppercase()))
}

/// A function name from the definition is written into SQL, so it must be one word.
fn check_word(function: &str) -> Result<(), Error> {
    if !function.is_empty() && function.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
        Ok(())
    } else {
        Err(Error::new(format!("alias function {function} is not supported")))
    }
}

/// A `complex-alias` operator: arithmetic, `||` concatenation, or `,` for the arguments of an
/// enclosing alias function such as `concat_ws`.
fn check_operator(operator: &str) -> Result<(), Error> {
    if matches!(operator.trim(), "+" | "-" | "*" | "/" | "%" | "||" | ",") {
        Ok(())
    } else {
        Err(Error::new(format!(
            "complex-alias operator {operator} is not supported"
        )))
    }
}

fn is_word(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

/// Java `EntityJavaUtil.camelCaseToUnderscored`.
fn camel_case_to_underscored(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (index, ch) in name.chars().enumerate() {
        if index > 0 && ch.is_uppercase() {
            out.push('_');
        }
        out.extend(ch.to_uppercase());
    }
    out
}
