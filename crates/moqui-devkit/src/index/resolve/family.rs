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

//! What takes a name from an entity field, a parameter or a form field (plans/design-dev-aide.md 13.6): for the LSP's
//! rename, through the expansions' declarations (`expand.rs`).

use super::expand::Mark;
use super::Resolver;
use crate::index::extract::{parameter_id, parse_parameter_id};

/// What takes a name from an entity field, parameter or form field ([`Resolver::same_name_dependents`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NameDependents {
    /// The ids of the artifacts of the same name that come from it or merge with it.
    pub ids: Vec<String>,
    /// What reads it by another name, or a name a rename cannot follow, each said in words.
    pub blocking: Vec<String>,
}

impl Resolver {
    /// What takes the name of the entity field, parameter or form field `id` from it, one step on (for a rename,
    /// [`NameDependents`]): of each service, form, transition, REST method or view entity that expands from its entity,
    /// service or form, the row of its name it expands (an `auto-parameters`' parameter, an `auto-fields` form field, an
    /// `alias-all`'s alias, a parameter from an implemented or called service, a field of a form it extends), and the one
    /// it declares of that name, which merges with it.
    #[must_use]
    pub fn same_name_dependents(&self, id: &str) -> NameDependents {
        let mut found = NameDependents::default();
        // the entity, service or form the name is of, the name, and the parameter's direction
        let (parent, name, direction) = if let Some(field) = id.strip_prefix("field:") {
            let Some((entity, name)) = field.rsplit_once('.') else {
                return found;
            };
            (format!("entity:{entity}"), name, None)
        } else if let Some((owner, direction, name)) = parse_parameter_id(id) {
            if name.contains('.') {
                return found;
            }
            (owner.to_string(), name, Some(direction))
        } else if let Some((form, name)) = id.strip_prefix("form-field:").and_then(|field| field.rsplit_once('@')) {
            (format!("form:{form}"), name, None)
        } else {
            return found;
        };
        let entity = parent.strip_prefix("entity:");
        // an entity's derived entity-auto services take its fields as parameters
        if let Some(entity) = entity {
            let mut derived: Vec<&String> = self
                .derived
                .keys()
                .filter(|service| service.rsplit_once('#').is_some_and(|(_, of)| of == entity))
                .collect();
            derived.sort();
            for service in derived {
                for row in &self.derived[service] {
                    if parse_parameter_id(&row.id).is_some_and(|(_, _, row_name)| row_name == name) {
                        found.ids.push(row.id.clone());
                    }
                }
            }
        }
        let mut readers: Vec<&String> = self
            .owners
            .keys()
            .filter(|owner| self.is_expandable(owner) && self.sources_of(owner).contains(&parent))
            .collect();
        readers.sort();
        for owner in readers {
            let mut add = |id: String| {
                if !found.ids.contains(&id) {
                    found.ids.push(id);
                }
            };
            // a parameter keeps its direction into a transition, REST method or implementing service; an entity's field
            // is either
            let direction_matches = |of: &str| direction.is_none_or(|direction| direction == of);
            for row in self.expanded_rows(owner) {
                match row.kind.as_str() {
                    "parameter" => {
                        if let Some((_, of, row_name)) = parse_parameter_id(&row.id) {
                            if row_name == name && direction_matches(of) {
                                add(row.id.clone());
                            }
                        }
                    }
                    "form-field" | "field" => {
                        let row_name = row
                            .id
                            .rsplit_once('@')
                            .or_else(|| row.id.rsplit_once('.'))
                            .map_or("", |(_, name)| name);
                        if row_name == name {
                            add(row.id.clone());
                        }
                    }
                    _ => {}
                }
            }
            for param in self.params.get(owner).into_iter().flatten() {
                if param.name == name && direction_matches(&param.direction) {
                    add(parameter_id(owner, &param.direction, name));
                } else if param.name == name && param.direction == "path" && direction == Some("in") {
                    found
                        .blocking
                        .push(format!("the path parameter {{{name}}} of {owner} is bound into it"));
                }
            }
            if let Some(form) = owner.strip_prefix("form:") {
                if self
                    .form_fields
                    .get(owner)
                    .into_iter()
                    .flatten()
                    .any(|field| field.name == name)
                {
                    add(format!("form-field:{form}@{name}"));
                }
            }
            // a view: an alias of the field's name reads it by that name; one that names it in `field`, or a prefixed
            // alias-all's, takes another name the rename cannot follow
            if let (Some(view), Some(entity)) = (owner.strip_prefix("entity:"), entity) {
                let marks = self.marks_of(owner);
                let members: Vec<&str> = marks
                    .iter()
                    .filter_map(|mark| match mark {
                        Mark::Member { alias, entity: member }
                            if self.entity(member).as_deref().unwrap_or(member) == entity =>
                        {
                            Some(*alias)
                        }
                        _ => None,
                    })
                    .collect();
                for alias in self.aliases.get(view).into_iter().flatten() {
                    if !alias
                        .entity_alias
                        .as_deref()
                        .is_some_and(|member| members.contains(&member))
                    {
                        continue;
                    }
                    match alias.field.as_deref() {
                        Some(field) if field == name => found
                            .blocking
                            .push(format!("the alias {} of {view} names it in its field", alias.name)),
                        None if alias.name == name => add(format!("field:{view}.{name}")),
                        _ => {}
                    }
                }
                for mark in &marks {
                    if let Mark::AliasAll {
                        alias,
                        prefix: Some(prefix),
                        excludes,
                    } = mark
                    {
                        if members.contains(alias) && !excludes.contains(&name) {
                            found.blocking.push(format!(
                                "{view} aliases it as {} by a prefixed alias-all",
                                moqui_db::model::prefixed_name(prefix, name)
                            ));
                        }
                    }
                }
            }
        }
        found
    }
}
