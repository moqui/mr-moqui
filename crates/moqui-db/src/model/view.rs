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

//! View-entity members and aliases, and the expansion Java does when it builds the definition.
//!
//! [`EntityModel::complete_view`] resolves `member-relationship` into a member, expands `alias-all`
//! into aliases, and copies type, primary key, encrypt, and localization from each member field.
//! After that, `member_entities` and `aliases` are the whole view, in document order. An alias with `pq-expression`
//! is in `pq_aliases` instead: it is not selected, and the value layer evaluates it on each row.

use std::collections::HashSet;

use moqui_lib::error::Error;
use moqui_lib::node::MNode;

use super::condition::EntityConditionDef;
use super::{is_true, located, optional_attr, parse_key_maps, require_attr, EntityDefinition, EntityModel, KeyMap};

/// Functions that aggregate, so the other selected columns go in `GROUP BY`.
pub const AGGREGATE_FUNCTIONS: [&str; 6] = ["min", "max", "sum", "avg", "count", "count-distinct"];

/// `sub-select` on a member. `On` is `LATERAL` when the database's `from-lateral-style` is `lateral`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubSelect {
    #[default]
    Off,
    On,
    NonLateral,
}

impl SubSelect {
    pub fn is_on(self) -> bool {
        self != Self::Off
    }
}

/// A view member. A `member-relationship` is one of these with `relationship` set; `entity_name`
/// and `key_map` are filled from that relationship when the view is completed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MemberEntity {
    pub entity_alias: String,
    pub entity_name: String,
    pub join_from_alias: Option<String>,
    pub join_optional: bool,
    pub key_map: Vec<KeyMap>,
    pub sub_select: SubSelect,
    /// Extra join conditions, written in this member's `ON` clause.
    pub entity_condition: Option<EntityConditionDef>,
    pub relationship: Option<String>,
}

impl MemberEntity {
    pub fn new(entity_alias: impl Into<String>, entity_name: impl Into<String>) -> Self {
        Self {
            entity_alias: entity_alias.into(),
            entity_name: entity_name.into(),
            ..Self::default()
        }
    }
}

/// What an alias selects.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AliasExpr {
    /// The member's `field`, or the alias name when `field` is absent.
    #[default]
    Column,
    Complex(ComplexAlias),
    Case(CaseAlias),
}

/// `complex-alias`: `expression`, then the parts joined by `operator`, or passed to `function`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ComplexAlias {
    pub operator: Option<String>,
    pub function: Option<String>,
    /// Raw SQL. `${}` is expanded per find.
    pub expression: Option<String>,
    pub parts: Vec<ComplexPart>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComplexPart {
    Alias(ComplexAlias),
    Field(ComplexField),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComplexField {
    pub entity_alias: String,
    pub field: String,
    pub function: Option<String>,
    /// `COALESCE(column, default_value)`. Raw SQL.
    pub default_value: Option<String>,
}

/// `case`: `CASE expression WHEN ... THEN ... ELSE ... END`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CaseAlias {
    pub expression: Option<String>,
    pub whens: Vec<(String, ComplexAlias)>,
    pub otherwise: Option<ComplexAlias>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FieldAlias {
    pub entity_alias: Option<String>,
    pub name: String,
    pub field: Option<String>,
    pub function: Option<String>,
    /// Declared, or the member field's type once the view is completed.
    pub field_type: Option<String>,
    /// Dictionary Java type of `field_type`. Empty when there is no type.
    pub java_type: String,
    pub is_aggregate: Option<bool>,
    pub expr: AliasExpr,
    pub is_pk: bool,
    pub encrypt: bool,
    pub enable_localization: bool,
    pub from_alias_all: bool,
    /// Full name of the member entity a column alias reads. Set when the view is completed.
    pub member_entity: Option<String>,
    /// `default-display`: whether a form-list's auto fields show the alias. `None` leaves it to the type.
    pub default_display: Option<bool>,
}

impl FieldAlias {
    pub fn column(name: impl Into<String>, entity_alias: impl Into<String>, field: Option<String>) -> Self {
        Self {
            name: name.into(),
            entity_alias: Some(entity_alias.into()),
            field,
            ..Self::default()
        }
    }

    /// The member field a column alias reads.
    pub fn field_name(&self) -> &str {
        self.field.as_deref().unwrap_or(self.name.as_str())
    }

    /// `is-aggregate`, or a `function` in [`AGGREGATE_FUNCTIONS`].
    pub fn aggregates(&self) -> bool {
        self.is_aggregate.unwrap_or_else(|| {
            self.function
                .as_deref()
                .is_some_and(|function| AGGREGATE_FUNCTIONS.contains(&function))
        })
    }

    /// Every member alias the expression reads.
    pub fn entity_aliases_used<'a>(&'a self) -> Vec<&'a str> {
        let mut out: Vec<&str> = Vec::new();
        if let Some(alias) = self.entity_alias.as_deref() {
            out.push(alias);
        }
        let mut push = |field: &'a ComplexField| {
            if !out.contains(&field.entity_alias.as_str()) {
                out.push(field.entity_alias.as_str());
            }
        };
        match &self.expr {
            AliasExpr::Column => {}
            AliasExpr::Complex(complex) => complex.visit_fields(&mut push),
            AliasExpr::Case(case) => {
                for (_, then) in &case.whens {
                    then.visit_fields(&mut push);
                }
                if let Some(otherwise) = &case.otherwise {
                    otherwise.visit_fields(&mut push);
                }
            }
        }
        out
    }

    /// The member this alias reads directly: `entity-alias` on a column alias. A condition on such
    /// an alias moves into that member's sub-select.
    pub fn direct_member(&self) -> Option<&str> {
        match self.expr {
            AliasExpr::Column => self.entity_alias.as_deref(),
            _ => None,
        }
    }
}

impl ComplexAlias {
    pub fn visit_fields<'a>(&'a self, visit: &mut impl FnMut(&'a ComplexField)) {
        for part in &self.parts {
            match part {
                ComplexPart::Alias(inner) => inner.visit_fields(visit),
                ComplexPart::Field(field) => visit(field),
            }
        }
    }
}

/// An alias with `pq-expression`, as Java's `pqExpressionNodeMap`. Java evaluates the expression with a row's values
/// in `EntityValueBase.get`; here moqui-db keeps it and the value layer evaluates it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PqAlias {
    pub name: String,
    pub expression: String,
    /// `default-display="true"`: a form's auto fields show it, as `ScreenForm.addEntityFields`.
    pub default_display: bool,
}

/// `alias-all` as written. Completion turns it into aliases.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AliasAll {
    pub entity_alias: String,
    pub prefix: Option<String>,
    pub exclude: Vec<String>,
}

pub(super) fn parse_member(node: &MNode) -> Result<MemberEntity, Error> {
    let relationship = if node.has_name("member-relationship") {
        Some(require_attr(node, "relationship")?)
    } else {
        None
    };
    Ok(MemberEntity {
        entity_alias: require_attr(node, "entity-alias")?,
        entity_name: if relationship.is_some() {
            String::new()
        } else {
            require_attr(node, "entity-name")?
        },
        join_from_alias: if relationship.is_some() {
            Some(require_attr(node, "join-from-alias")?)
        } else {
            optional_attr(node, "join-from-alias")
        },
        join_optional: is_true(node, "join-optional"),
        key_map: parse_key_maps(node)?,
        sub_select: match node.attribute("sub-select").as_deref() {
            Some("true") => SubSelect::On,
            Some("non-lateral") => SubSelect::NonLateral,
            _ => SubSelect::Off,
        },
        entity_condition: node
            .first_named("entity-condition")
            .map(|condition| EntityConditionDef::parse(&condition))
            .transpose()?,
        relationship,
    })
}

/// The alias as a [`PqAlias`] when it has `pq-expression`; then Java checks none of its other attributes.
pub(super) fn parse_pq_alias(node: &MNode) -> Result<Option<PqAlias>, Error> {
    let Some(expression) = optional_attr(node, "pq-expression") else {
        return Ok(None);
    };
    Ok(Some(PqAlias {
        name: require_attr(node, "name")?,
        expression,
        default_display: is_true(node, "default-display"),
    }))
}

pub(super) fn parse_alias(node: &MNode, dictionary: &HashSet<String>) -> Result<FieldAlias, Error> {
    let name = require_attr(node, "name")?;
    let field_type = optional_attr(node, "type");
    if let Some(field_type) = &field_type {
        if !dictionary.contains(field_type) {
            return Err(located(
                node,
                format!("alias {name} type {field_type} is not a dictionary type"),
            ));
        }
    }
    let expr = if let Some(case) = node.first_named("case") {
        AliasExpr::Case(parse_case(&case, &name)?)
    } else if let Some(complex) = node.first_named("complex-alias") {
        AliasExpr::Complex(parse_complex(&complex)?)
    } else {
        AliasExpr::Column
    };
    let entity_alias = optional_attr(node, "entity-alias");
    if expr == AliasExpr::Column && entity_alias.is_none() {
        return Err(located(node, format!("alias {name} has no entity-alias")));
    }
    Ok(FieldAlias {
        entity_alias,
        name,
        field: optional_attr(node, "field"),
        function: optional_attr(node, "function"),
        field_type,
        is_aggregate: optional_attr(node, "is-aggregate").map(|value| value.eq_ignore_ascii_case("true")),
        expr,
        default_display: optional_attr(node, "default-display").map(|value| value == "true"),
        ..FieldAlias::default()
    })
}

fn parse_complex(node: &MNode) -> Result<ComplexAlias, Error> {
    let mut parts = Vec::new();
    for child in node.children() {
        match child.name().as_str() {
            "complex-alias" => parts.push(ComplexPart::Alias(parse_complex(&child)?)),
            "complex-alias-field" => parts.push(ComplexPart::Field(ComplexField {
                entity_alias: require_attr(&child, "entity-alias")?,
                field: require_attr(&child, "field")?,
                function: optional_attr(&child, "function"),
                default_value: optional_attr(&child, "default-value"),
            })),
            _ => {}
        }
    }
    Ok(ComplexAlias {
        operator: optional_attr(node, "operator"),
        function: optional_attr(node, "function"),
        expression: node.attribute("expression").filter(|value| !value.is_empty()),
        parts,
    })
}

fn parse_case(node: &MNode, alias_name: &str) -> Result<CaseAlias, Error> {
    let mut whens = Vec::new();
    for when in node.children_named("when") {
        let complex = when.first_named("complex-alias").ok_or_else(|| {
            located(
                &when,
                format!("no complex-alias element under case.when in alias {alias_name}"),
            )
        })?;
        whens.push((require_attr(&when, "expression")?, parse_complex(&complex)?));
    }
    if whens.is_empty() {
        return Err(located(
            node,
            format!("no when element under case in alias {alias_name}"),
        ));
    }
    let otherwise = match node.first_named("else") {
        Some(otherwise) => {
            let complex = otherwise.first_named("complex-alias").ok_or_else(|| {
                located(
                    &otherwise,
                    format!("no complex-alias element under case.else in alias {alias_name}"),
                )
            })?;
            Some(parse_complex(&complex)?)
        }
        None => None,
    };
    Ok(CaseAlias {
        expression: optional_attr(node, "expression"),
        whens,
        otherwise,
    })
}

pub(super) fn parse_alias_all(node: &MNode) -> Result<AliasAll, Error> {
    let exclude = node
        .children_named("exclude")
        .iter()
        .map(|child| require_attr(child, "field"))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AliasAll {
        entity_alias: require_attr(node, "entity-alias")?,
        prefix: optional_attr(node, "prefix"),
        exclude,
    })
}

/// `prefix` plus the field name with its first letter upper-cased.
pub fn prefixed_name(prefix: &str, field: &str) -> String {
    let mut name = String::with_capacity(prefix.len() + field.len());
    name.push_str(prefix);
    let mut chars = field.chars();
    if let Some(first) = chars.next() {
        name.push(first.to_ascii_uppercase());
        name.push_str(chars.as_str());
    }
    name
}

impl EntityModel {
    /// Finish a view the way `EntityDefinition` does when it is built.
    ///
    /// Members named by short entity name get the full name. A `member-relationship` member gets
    /// the related entity and the relationship's expanded key map. `alias-all` becomes aliases,
    /// after the declared ones, skipping `lastUpdatedStamp`, excluded fields, and names already
    /// used, a `pq-expression` alias's among them. Each column alias takes the member field's type, primary key, encrypt, and
    /// localization flags. The view takes its group from the first member when it has none, and
    /// is never cached when a member is never cached. Running it twice changes nothing.
    pub fn complete_view(&self, view: &mut EntityDefinition) -> Result<(), Error> {
        if !view.is_view {
            return Ok(());
        }
        self.resolve_members(view)?;
        self.expand_alias_alls(view)?;
        self.derive_aliases(view)?;
        if view.group.is_none() {
            if let Some(first) = view.member_entities.first() {
                view.group = self.entity(&first.entity_name)?.group.clone();
            }
        }
        for member in &view.member_entities {
            if self.entity(&member.entity_name)?.cache == "never" {
                view.cache = "never".to_string();
            }
        }
        Ok(())
    }

    pub(super) fn resolve_members(&self, view: &mut EntityDefinition) -> Result<(), Error> {
        for index in 0..view.member_entities.len() {
            let member = &view.member_entities[index];
            if let Some(relationship_name) = member.relationship.clone() {
                if !member.entity_name.is_empty() {
                    continue;
                }
                let join_from = member.join_from_alias.clone().unwrap_or_default();
                let from_name = view.member_entities[..index]
                    .iter()
                    .find(|other| other.entity_alias == join_from)
                    .map(|other| other.entity_name.clone())
                    .ok_or_else(|| {
                        Error::new(format!(
                            "could not find member-entity {join_from} referenced in member-relationship {} of view-entity {}",
                            member.entity_alias, view.full_name
                        ))
                    })?;
                let from = self.entity(&from_name)?;
                let relationship = from.relationship(&relationship_name).ok_or_else(|| {
                    Error::new(format!(
                        "could not find relationship {relationship_name} from member-entity {join_from} referenced in member-relationship {} of view-entity {}",
                        member.entity_alias, view.full_name
                    ))
                })?;
                let entity_name = relationship.related.clone();
                let key_map = relationship
                    .key_pairs
                    .iter()
                    .map(|(field_name, related)| KeyMap {
                        field_name: field_name.clone(),
                        related: Some(related.clone()),
                    })
                    .collect();
                let member = &mut view.member_entities[index];
                member.entity_name = entity_name;
                member.key_map = key_map;
            } else {
                let full_name = self
                    .entity(&member.entity_name)
                    .map_err(|err| {
                        Error::new(format!(
                            "no definition found for member-entity {} name {} in view-entity {}: {err}",
                            member.entity_alias, member.entity_name, view.full_name
                        ))
                    })?
                    .full_name
                    .clone();
                view.member_entities[index].entity_name = full_name;
            }
        }
        Ok(())
    }

    fn expand_alias_alls(&self, view: &mut EntityDefinition) -> Result<(), Error> {
        let mut used: HashSet<String> = view
            .aliases
            .iter()
            .map(|alias| alias.name.clone())
            .chain(view.pq_aliases.iter().map(|alias| alias.name.clone()))
            .collect();
        let mut added = Vec::new();
        for alias_all in &view.alias_alls {
            let member = view.member(&alias_all.entity_alias).ok_or_else(|| {
                Error::new(format!(
                    "view {} alias-all {} has no member-entity",
                    view.full_name, alias_all.entity_alias
                ))
            })?;
            let member_entity = self.entity(&member.entity_name)?;
            for field_name in member_entity.field_names() {
                if field_name == "lastUpdatedStamp" || alias_all.exclude.iter().any(|name| name == field_name) {
                    continue;
                }
                let name = match alias_all.prefix.as_deref() {
                    Some(prefix) => prefixed_name(prefix, field_name),
                    None => field_name.to_string(),
                };
                if !used.insert(name.clone()) {
                    continue;
                }
                let mut alias = FieldAlias::column(
                    name.clone(),
                    alias_all.entity_alias.clone(),
                    (name != field_name).then(|| field_name.to_string()),
                );
                alias.from_alias_all = true;
                added.push(alias);
            }
        }
        // Completion may run again on a finished view. Aliases added the first time are already
        // in `used`, so nothing is added twice.
        view.aliases.extend(added);
        Ok(())
    }

    fn derive_aliases(&self, view: &mut EntityDefinition) -> Result<(), Error> {
        let members = view.member_entities.clone();
        let full_name = view.full_name.clone();
        let member_of = |alias: &str| -> Result<&EntityDefinition, Error> {
            let member = members
                .iter()
                .find(|member| member.entity_alias == alias)
                .ok_or_else(|| {
                    Error::new(format!(
                        "could not find member-entity with entity-alias {alias} in view-entity {full_name}"
                    ))
                })?;
            self.entity(&member.entity_name)
        };
        for alias in &mut view.aliases {
            for entity_alias in alias.entity_aliases_used() {
                member_of(entity_alias)?;
            }
            if alias.expr == AliasExpr::Column {
                let entity_alias = alias.entity_alias.clone().unwrap_or_default();
                let member = member_of(&entity_alias)?;
                let field_name = alias.field_name().to_string();
                let Some(source) = member.view_field(&field_name) else {
                    return Err(Error::new(format!(
                        "in view-entity {full_name} alias {} referred to field {field_name} that does not exist on entity {}",
                        alias.name, member.full_name
                    )));
                };
                if alias.field_type.is_none() {
                    alias.field_type = source.field_type;
                }
                alias.is_pk = source.is_pk;
                alias.encrypt = source.encrypt;
                alias.enable_localization = source.enable_localization;
                alias.member_entity = Some(member.full_name.clone());
            }
            alias.java_type = alias
                .field_type
                .as_deref()
                .and_then(|field_type| self.dictionary_type(field_type))
                .map(|entry| entry.java_type.clone())
                .unwrap_or_default();
        }
        Ok(())
    }
}

/// The parts of a field or alias that a view alias copies.
pub(super) struct SourceField {
    pub field_type: Option<String>,
    pub is_pk: bool,
    pub encrypt: bool,
    pub enable_localization: bool,
}
