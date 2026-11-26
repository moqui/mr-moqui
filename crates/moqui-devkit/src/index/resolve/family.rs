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
//! rename, through the expansions (`expand.rs`), each row followed by what it was made from (its provenance), not by
//! its name alone: a view's alias of another member's field of that name, or a declared parameter an `include="pk"`
//! leaves apart from a field, is not of the family.

use std::collections::HashSet;

use super::expand::{Mark, ViewField};
use super::{entity_auto, Resolver};
use crate::index::extract::{parameter_id, parse_parameter_id};

/// What takes a name from an entity field, parameter or form field ([`Resolver::same_name_dependents`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NameDependents {
    /// The ids of the artifacts of the same name that come from it or merge with it.
    pub ids: Vec<String>,
    /// What reads it by another name, or a name a rename cannot follow, each said in words.
    pub blocking: Vec<String>,
    /// The attributes that name it elsewhere and are edited with it ([`NameEdit`]).
    pub edits: Vec<NameEdit>,
}

/// An attribute that names a field or parameter of a family and is edited with it: of the start tag of `artifact`'s
/// element (a view alias's `field`), or, with `exclude_of`, of an `exclude` in `artifact`'s element under the auto
/// element that names it (an `auto-parameters`' or `auto-fields-entity`'s `field-name`, an `auto-fields-service`'s
/// `parameter-name`, an `alias-all`'s `field`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameEdit {
    pub artifact: String,
    pub attribute: String,
    pub exclude_of: Option<ExcludeOf>,
}

/// The auto element an `exclude` is in: its name, and its attribute naming what it reads with that attribute's value
/// as written (none: the attribute is not written, an `auto-parameters` of the service's noun).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExcludeOf {
    pub element: String,
    pub key_attribute: String,
    pub key: Option<String>,
}

impl Resolver {
    /// What takes the name of the entity field, parameter or form field `id` from it, one step on (for a rename,
    /// [`NameDependents`]): of each service, form, transition, REST method or view entity that expands from its entity,
    /// service or form, each row made from it (its provenance names `id`) whose name is its name: an
    /// `auto-parameters`' parameter (or the declared one it merged into), an `auto-fields` form field, an `alias-all`'s
    /// alias, a parameter from an implemented or called service, a field of a form it extends; an entity's derived
    /// entity-auto parameters; the excludes that name it; and what reads it by another name.
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
        let add = |found: &mut NameDependents, id: String| {
            if !found.ids.contains(&id) {
                found.ids.push(id);
            }
        };
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
                        add(&mut found, row.id.clone());
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
            let kind = self.owner(owner).map(|info| info.kind.as_str()).unwrap_or_default();
            if kind == "view-entity" {
                self.view_dependents(owner, id, &mut found);
            } else {
                for (row, row_name, from) in self.provenance(owner, kind) {
                    if row_name == name && from.iter().any(|source| source == id) {
                        add(&mut found, row);
                    }
                }
            }
            // a REST method's path parameter is bound into the service's in-parameter of its name
            if kind == "rest" && direction == Some("in") {
                let bound = self
                    .params
                    .get(owner)
                    .into_iter()
                    .flatten()
                    .any(|param| param.direction == "path" && param.name == name);
                if bound {
                    found
                        .blocking
                        .push(format!("the path parameter {{{name}}} of {owner} is bound into it"));
                }
            }
            self.exclude_edits(owner, &parent, name, &mut found);
        }
        found
    }

    /// Whether the service `service` (its id) reads its noun as an entity: an `auto-parameters` of its own with no
    /// `entity-name` (an implemented service's reads that service's noun, as Java expands it there).
    #[must_use]
    pub fn reads_noun(&self, service: &str) -> bool {
        self.marks_of(service)
            .iter()
            .any(|mark| matches!(mark, Mark::AutoParameters { entity: None, .. }))
    }

    /// Each row the owner `owner` (of `kind`) expands, with its name and what it was made from.
    fn provenance(&self, owner: &str, kind: &str) -> Vec<(String, String, Vec<String>)> {
        let mut rows = Vec::new();
        match kind {
            "service" => {
                let noun = owner.rsplit_once('#').map(|(_, noun)| noun);
                for direction in ["in", "out"] {
                    let mut visited = HashSet::from([owner.to_string()]);
                    for param in self.full_params(owner, direction, &mut visited, 0, noun) {
                        rows.push((parameter_id(owner, direction, &param.name), param.name, param.from));
                    }
                }
            }
            "transition" | "rest" => {
                let marks = self.marks_of(owner);
                let Some(service) = Self::called_service(&marks) else {
                    return rows;
                };
                let passes = kind == "rest"
                    || marks.iter().any(
                        |mark| matches!(mark, Mark::Call { service: called, passes: true, .. } if *called == service),
                    );
                let target = format!("service:{service}");
                let noun = service.rsplit_once('#').map(|(_, noun)| noun);
                for direction in ["in", "out"] {
                    if direction == "in" && !passes {
                        continue;
                    }
                    let mut visited = HashSet::from([owner.to_string(), target.clone()]);
                    for param in self.full_params(&target, direction, &mut visited, 0, noun) {
                        if param.required == "disabled" {
                            continue;
                        }
                        // the transition's own parameter of the name has the id its service's would have
                        let source = parameter_id(&target, direction, &param.name);
                        rows.push((parameter_id(owner, direction, &param.name), param.name, vec![source]));
                    }
                }
            }
            "form" => {
                let key = owner.strip_prefix("form:").unwrap_or(owner);
                let mut visited = HashSet::from([owner.to_string()]);
                for field in self.full_fields(owner, &mut visited, 0) {
                    rows.push((format!("form-field:{key}@{}", field.name), field.name, field.from));
                }
            }
            _ => {}
        }
        rows
    }

    /// The dependents in the view `view` (its id) of the field `id`: an alias that reads it by its name
    /// (an `alias-all`'s, or one with no `field`), the `field` of an alias that names it, and a prefixed `alias-all`'s,
    /// which a rename cannot follow.
    fn view_dependents(&self, view: &str, id: &str, found: &mut NameDependents) {
        let full = view.strip_prefix("entity:").unwrap_or(view);
        for ViewField {
            field,
            from_alias_all,
            from,
            by_other_name,
        } in self.view_fields(view, 0)
        {
            if from.as_deref() != Some(id) {
                continue;
            }
            let alias = format!("field:{full}.{}", field.name);
            match (from_alias_all, by_other_name) {
                (_, false) => {
                    if !found.ids.contains(&alias) {
                        found.ids.push(alias);
                    }
                }
                (false, true) => found.edits.push(NameEdit {
                    artifact: alias,
                    attribute: "field".to_string(),
                    exclude_of: None,
                }),
                (true, true) => found
                    .blocking
                    .push(format!("{full} aliases it as {} by a prefixed alias-all", field.name)),
            }
        }
    }

    /// The excludes of the owner `owner` that name `name` of the entity or service `parent`, each an edit.
    fn exclude_edits(&self, owner: &str, parent: &str, name: &str, found: &mut NameDependents) {
        let marks = self.marks_of(owner);
        let entity_of = |named: &str| format!("entity:{}", self.entity(named).unwrap_or_else(|| named.to_string()));
        let members = self.view_members(&marks);
        let mut edit = |element: &str, key_attribute: &str, key: Option<&str>, attribute: &str| {
            found.edits.push(NameEdit {
                artifact: owner.to_string(),
                attribute: attribute.to_string(),
                exclude_of: Some(ExcludeOf {
                    element: element.to_string(),
                    key_attribute: key_attribute.to_string(),
                    key: key.map(str::to_string),
                }),
            });
        };
        for mark in &marks {
            match mark {
                Mark::AutoParameters { entity, excludes, .. } if excludes.contains(&name) => {
                    let read = entity.or_else(|| owner.rsplit_once('#').map(|(_, noun)| noun));
                    if read.is_some_and(|read| entity_of(read) == parent) {
                        edit("auto-parameters", "entity-name", *entity, "field-name");
                    }
                }
                Mark::AutoFieldsEntity { entity, excludes, .. }
                    if excludes.contains(&name) && entity_of(entity) == parent =>
                {
                    edit("auto-fields-entity", "entity-name", Some(entity), "field-name");
                }
                Mark::AutoFieldsService { service, excludes, .. }
                    if excludes.contains(&name)
                        && format!("service:{service}") == parent
                        && entity_auto(service).is_none() =>
                {
                    edit("auto-fields-service", "service-name", Some(service), "parameter-name");
                }
                Mark::AliasAll { alias, excludes, .. } if excludes.contains(&name) => {
                    let reads = members.iter().any(|(member, entity)| {
                        member == alias && entity.as_ref().is_some_and(|full| format!("entity:{full}") == parent)
                    });
                    if reads {
                        edit("alias-all", "entity-alias", Some(alias), "field");
                    }
                }
                _ => {}
            }
        }
    }
}
