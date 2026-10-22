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

//! `entity-condition` on a view-entity or on one of its members. Parsed at load, evaluated per find.

use moqui_lib::error::Error;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use super::{optional_attr, require_attr, EntityDefinition, EntityModel, MemberEntity};
use crate::sql::{comparison, Condition, ConditionValues, FieldRef};

/// `entity-condition`. On a member-entity only `conditions` is used, in the join's `ON` clause.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EntityConditionDef {
    pub distinct: bool,
    pub conditions: ConditionGroup,
    pub having: Option<ConditionGroup>,
    /// `order-by` field names, appended after the find's own order.
    pub order_by: Vec<String>,
}

/// `entity-condition`, `econditions`, or `having-econditions`: the items joined by `combine`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConditionGroup {
    pub or: bool,
    pub items: Vec<ConditionItem>,
}

/// Items in the order Java builds them: every `date-filter`, then every `econdition`, then every `econditions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConditionItem {
    DateFilter(DateFilter),
    Compare(ECondition),
    Group(ConditionGroup),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateFilter {
    pub entity_alias: Option<String>,
    pub from_field_name: String,
    pub thru_field_name: String,
    /// Expanded and converted to a timestamp. Absent means now.
    pub valid_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ECondition {
    pub entity_alias: Option<String>,
    pub field_name: String,
    pub operator: String,
    pub to_entity_alias: Option<String>,
    pub to_field_name: Option<String>,
    pub value: Option<String>,
    pub ignore_case: bool,
    pub or_null: bool,
}

impl EntityConditionDef {
    pub(crate) fn parse(node: &MNode) -> Result<Self, Error> {
        Ok(Self {
            distinct: node.attribute_is("distinct", "true"),
            conditions: ConditionGroup::parse(node)?,
            having: node
                .first_named("having-econditions")
                .map(|having| ConditionGroup::parse(&having))
                .transpose()?,
            order_by: node
                .children_named("order-by")
                .iter()
                .map(|order| require_attr(order, "field-name"))
                .collect::<Result<_, _>>()?,
        })
    }
}

impl ConditionGroup {
    fn parse(node: &MNode) -> Result<Self, Error> {
        let mut items = Vec::new();
        for filter in node.children_named("date-filter") {
            items.push(ConditionItem::DateFilter(DateFilter {
                entity_alias: optional_attr(&filter, "entity-alias"),
                from_field_name: optional_attr(&filter, "from-field-name").unwrap_or_else(|| "fromDate".to_string()),
                thru_field_name: optional_attr(&filter, "thru-field-name").unwrap_or_else(|| "thruDate".to_string()),
                valid_date: optional_attr(&filter, "valid-date"),
            }));
        }
        for econdition in node.children_named("econdition") {
            items.push(ConditionItem::Compare(ECondition {
                entity_alias: optional_attr(&econdition, "entity-alias"),
                field_name: require_attr(&econdition, "field-name")?,
                operator: optional_attr(&econdition, "operator").unwrap_or_else(|| "equals".to_string()),
                to_entity_alias: optional_attr(&econdition, "to-entity-alias"),
                to_field_name: optional_attr(&econdition, "to-field-name"),
                value: optional_attr(&econdition, "value"),
                ignore_case: econdition.attribute_is("ignore-case", "true"),
                or_null: econdition.attribute_is("or-null", "true"),
            }));
        }
        for group in node.children_named("econditions") {
            items.push(ConditionItem::Group(Self::parse(&group)?));
        }
        Ok(Self {
            or: node.attribute_is("combine", "or"),
            items,
        })
    }

    /// The condition this group stands for, or `None` when it has no items.
    ///
    /// A field with an entity alias names that member's field. Without one it names a view alias,
    /// except under a member-entity, where a field the joined entity has defaults to that member,
    /// as `EntityDefinition.makeViewListCondition` does.
    pub fn build(
        &self,
        view: &EntityDefinition,
        model: &EntityModel,
        join_member: Option<&MemberEntity>,
        values: &dyn ConditionValues,
    ) -> Result<Option<Condition>, Error> {
        let scope = Scope {
            view,
            model,
            join_member,
        };
        self.build_in(&scope, values)
    }

    fn build_in(&self, scope: &Scope<'_>, values: &dyn ConditionValues) -> Result<Option<Condition>, Error> {
        let mut list = Vec::new();
        for item in &self.items {
            let built = match item {
                ConditionItem::DateFilter(filter) => Some(filter.build(scope, values)?),
                ConditionItem::Compare(econdition) => Some(econdition.build(scope, values)?),
                ConditionItem::Group(group) => group.build_in(scope, values)?,
            };
            list.extend(built);
        }
        Ok(match list.len() {
            0 => None,
            1 => list.pop(),
            _ if self.or => Some(Condition::or(list)),
            _ => Some(Condition::and(list)),
        })
    }
}

impl DateFilter {
    fn build(&self, scope: &Scope<'_>, values: &dyn ConditionValues) -> Result<Condition, Error> {
        let alias = scope.default_alias(self.entity_alias.as_deref(), &self.from_field_name)?;
        let from = scope.field(alias.as_deref(), &self.from_field_name)?.0;
        let thru = scope.field(alias.as_deref(), &self.thru_field_name)?.0;
        let valid = match &self.valid_date {
            Some(text) => {
                let text = expand(values, text)?;
                values.convert(&text, "java.sql.Timestamp")?
            }
            None => Value::Null,
        };
        let valid = if valid.is_null() { values.now() } else { valid };
        Ok(Condition::and(vec![
            Condition::or(vec![
                Condition::is_null(from.clone()),
                Condition::less_equal(from, valid.clone()),
            ]),
            Condition::or(vec![Condition::is_null(thru.clone()), Condition::greater(thru, valid)]),
        ]))
    }
}

impl ECondition {
    fn build(&self, scope: &Scope<'_>, values: &dyn ConditionValues) -> Result<Condition, Error> {
        let alias = scope.default_alias(self.entity_alias.as_deref(), &self.field_name)?;
        let (field, java_type) = scope.field(alias.as_deref(), &self.field_name)?;
        let mut condition = if let Some(to_field_name) = &self.to_field_name {
            let to_alias = scope.default_alias(self.to_entity_alias.as_deref(), to_field_name)?;
            let (to_field, _) = scope.field(to_alias.as_deref(), to_field_name)?;
            let op = crate::sql::CompareOp::from_operator(&self.operator).ok_or_else(|| {
                Error::new(format!(
                    "econdition {} operator {} cannot compare two fields",
                    self.field_name, self.operator
                ))
            })?;
            Condition::compare_fields(field.clone(), op, to_field)
        } else {
            let value = match self.value.as_deref() {
                None => Value::Null,
                Some(text) => {
                    let text = expand(values, text)?;
                    convert_operand(values, &self.operator, &text, &java_type)?
                }
            };
            comparison(field.clone(), &self.operator, value)?
        };
        if self.ignore_case {
            condition = Condition::ignore_case(condition);
        }
        if self.or_null {
            condition = Condition::or(vec![condition, Condition::is_null(field)]);
        }
        Ok(condition)
    }
}

/// `in` and `between` take a comma list. Each part is converted on its own.
fn convert_operand(values: &dyn ConditionValues, operator: &str, text: &str, java_type: &str) -> Result<Value, Error> {
    if matches!(operator, "in" | "not-in" | "between" | "not-between") {
        let parts = text
            .split(',')
            .map(|part| convert(values, part, java_type))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Value::List(parts));
    }
    convert(values, text, java_type)
}

fn convert(values: &dyn ConditionValues, text: &str, java_type: &str) -> Result<Value, Error> {
    if java_type.is_empty() {
        return Ok(Value::string(text));
    }
    values.convert(text, java_type)
}

fn expand(values: &dyn ConditionValues, text: &str) -> Result<String, Error> {
    if text.contains("${") {
        values.expand(text)
    } else {
        Ok(text.to_string())
    }
}

struct Scope<'a> {
    view: &'a EntityDefinition,
    model: &'a EntityModel,
    join_member: Option<&'a MemberEntity>,
}

impl Scope<'_> {
    /// The entity alias a field uses: the one given, or the joined member when it has that field.
    fn default_alias(&self, given: Option<&str>, field_name: &str) -> Result<Option<String>, Error> {
        if let Some(given) = given {
            return Ok(Some(given.to_string()));
        }
        let Some(member) = self.join_member else {
            return Ok(None);
        };
        let entity = self.model.entity(&member.entity_name)?;
        if entity.has_field(field_name) {
            return Ok(Some(member.entity_alias.clone()));
        }
        Ok(None)
    }

    /// The field reference and its Java type.
    fn field(&self, entity_alias: Option<&str>, field_name: &str) -> Result<(FieldRef, String), Error> {
        match entity_alias {
            Some(entity_alias) => {
                let member = self.view.member(entity_alias).ok_or_else(|| {
                    Error::new(format!(
                        "the entity-alias {entity_alias} was not found in view-entity {}",
                        self.view.full_name
                    ))
                })?;
                let entity = self.model.entity(&member.entity_name)?;
                let java_type = entity.java_type_of(field_name).ok_or_else(|| {
                    Error::new(format!("field {field_name} not found in entity {}", entity.full_name))
                })?;
                Ok((FieldRef::aliased(entity_alias, field_name), java_type.to_string()))
            }
            None => {
                let java_type = self.view.java_type_of(field_name).ok_or_else(|| {
                    Error::new(format!(
                        "field {field_name} not found in entity {}",
                        self.view.full_name
                    ))
                })?;
                Ok((FieldRef::new(field_name), java_type.to_string()))
            }
        }
    }
}
