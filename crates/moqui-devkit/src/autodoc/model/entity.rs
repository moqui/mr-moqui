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

//! An entity's or view entity's doc: its attributes, fields (with the component of each `extend-entity` that adds one),
//! relationships, indexes, view members and aliases, its extensions, its ECA rules, and an E-R diagram of the entity and
//! the entities it relates to.

use std::collections::BTreeSet;

use moqui_db::model::EntityDefinition;
use moqui_lib::error::Error;
use serde_json::{json, Value as Json};

use super::Run;
use crate::autodoc::mermaid::{self, Cardinality, ErAttribute, ErEntity, ErLink};
use crate::autodoc::plan::DocEntry;
use crate::index::Direction;

pub(super) fn data(run: &mut Run<'_>, doc: &DocEntry) -> Result<Json, Error> {
    let facade = run.factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let definition = facade.with_entity(&doc.name, |entity, _, _| Ok(entity.clone()))?;
    let description = run.description_of(&doc.file, doc.line);
    let mut map = run.header(doc, description);
    let children = run.children_by_id(&doc.id)?;

    let fields: Vec<Json> = definition
        .fields
        .iter()
        .map(|field| {
            let row = children.get(&format!("field:{}.{}", definition.full_name, field.name));
            let description = row.and_then(|row| run.description_of(&row.path.clone(), row.line));
            let extension = row
                .filter(|row| row.component != doc.component)
                .map(|row| row.component.clone());
            json!({
                "name": field.name,
                "type": field.field_type,
                "pk": field.is_pk,
                "notNull": field.not_null,
                "default": field.default_value,
                "encrypt": field.encrypt,
                "createOnly": field.create_only,
                "description": description.unwrap_or_default(),
                "extension": extension,
            })
        })
        .collect();

    let relationships: Vec<Json> = definition
        .relationships
        .iter()
        .filter(|relationship| !relationship.is_auto_reverse)
        .map(|relationship| {
            let row = children.get(&format!("relationship:{}@{}", definition.full_name, relationship.name));
            let description = row.and_then(|row| run.description_of(&row.path.clone(), row.line));
            let related_doc = run.plan.of_id(&format!("entity:{}", relationship.related));
            json!({
                "type": relationship.relationship_type,
                "related": relationship.related,
                "relatedTitle": related_doc.map_or_else(|| short(&relationship.related), |found| found.title.clone()),
                "relatedHref": related_doc.and_then(|found| run.link(doc, found)),
                "title": relationship.title,
                "keyMap": relationship
                    .key_pairs
                    .iter()
                    .map(|(field, related)| if field == related { field.clone() } else { format!("{field} = {related}") })
                    .collect::<Vec<_>>()
                    .join(", "),
                "fk": relationship.fk_name,
                "shortAlias": relationship.short_alias,
                "description": description.unwrap_or_default(),
            })
        })
        .collect();

    let indexes: Vec<Json> = definition
        .indexes
        .iter()
        .map(|index| json!({"name": index.name, "unique": index.unique, "fields": index.fields.join(", ")}))
        .collect();

    let members: Vec<Json> = definition
        .member_entities
        .iter()
        .map(|member| {
            let member_doc = run.plan.of_id(&format!("entity:{}", member.entity_name)).or_else(|| {
                facade
                    .with_entity(&member.entity_name, |entity, _, _| Ok(entity.full_name.clone()))
                    .ok()
                    .and_then(|full| run.plan.of_id(&format!("entity:{full}")))
            });
            json!({
                "alias": member.entity_alias,
                "entity": member.entity_name,
                "href": member_doc.and_then(|found| run.link(doc, found)),
                "joinFrom": member.join_from_alias,
                "optional": member.join_optional,
                "relationship": member.relationship,
                "keyMap": member
                    .key_map
                    .iter()
                    .map(|key| match &key.related {
                        Some(related) if related != &key.field_name => format!("{} = {related}", key.field_name),
                        _ => key.field_name.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            })
        })
        .collect();

    let aliases: Vec<Json> = definition
        .aliases
        .iter()
        .filter(|alias| !alias.from_alias_all)
        .map(|alias| {
            json!({
                "name": alias.name,
                "entityAlias": alias.entity_alias,
                "field": alias.field,
                "function": alias.function,
                "pk": alias.is_pk,
            })
        })
        .collect();

    // the extend-entity files of other components, and the ECA rules on it
    let mut extensions: Vec<Json> = Vec::new();
    let mut rules: Vec<Json> = Vec::new();
    for entry in run.source.refs(&doc.id, Direction::In)? {
        let Some(other) = &entry.other else { continue };
        match other.kind.as_str() {
            "extend-entity" if entry.ref_type == "extend" => {
                extensions.push(json!({
                    "component": other.component,
                    "location": run.location(&other.component, &other.path),
                    "line": other.line,
                }));
            }
            "eeca" => {
                let file = run.plan.of_artifact(other);
                rules.push(json!({
                    "title": other.short_name,
                    "summary": other.summary,
                    "href": file.and_then(|found| run.link(doc, found)),
                }));
            }
            _ => {}
        }
    }

    let (used_by, uses, usage) = run.usage_data(doc, &[&doc.id])?;
    let neighbors = neighbor_diagram(&facade, &definition);
    map.insert(
        "entity".into(),
        json!({
            "fullName": definition.full_name,
            "entityName": definition.entity_name,
            "package": definition.package_name,
            "isView": definition.is_view,
            "group": definition.group,
            "table": definition.table_name,
            "cache": definition.cache,
            "use": definition.entity_use,
            "createOnly": definition.create_only,
            "audit": definition.enable_audit_log,
            "shortAlias": definition.short_alias,
            "pkFields": definition.pk_field_names().join(", "),
            "fields": fields,
            "relationships": relationships,
            "indexes": indexes,
            "members": members,
            "aliases": aliases,
            "extensions": extensions,
            "ecaRules": rules,
        }),
    );
    map.insert("usedBy".into(), used_by);
    map.insert("uses".into(), uses);
    map.insert("diagrams".into(), json!({"usage": usage, "relationships": neighbors}));
    Ok(Json::Object(map))
}

fn short(full: &str) -> String {
    full.rsplit('.').next().unwrap_or(full).to_string()
}

/// An E-R diagram of `definition` with its fields and the entities its relationships name, without theirs.
fn neighbor_diagram(facade: &moqui_core::entity::EntityFacade, definition: &EntityDefinition) -> String {
    let mut entities = vec![er_entity(definition, false)];
    let mut seen = BTreeSet::from([definition.full_name.clone()]);
    for relationship in &definition.relationships {
        if seen.insert(relationship.related.clone()) {
            let stub = facade
                .with_entity(&relationship.related, |entity, _, _| Ok(er_entity(entity, true)))
                .unwrap_or_else(|_| ErEntity {
                    full_name: relationship.related.clone(),
                    short_name: short(&relationship.related),
                    stub: true,
                    attributes: Vec::new(),
                });
            entities.push(stub);
        }
    }
    let links = er_links(facade, std::slice::from_ref(definition), &seen);
    if entities.len() == 1 && definition.fields.is_empty() {
        return String::new();
    }
    mermaid::er_diagram(&entities, &links)
}

/// An entity of an E-R diagram: its key fields, the foreign ones marked, unless a stub.
pub(super) fn er_entity(definition: &EntityDefinition, stub: bool) -> ErEntity {
    let foreign: BTreeSet<&str> = definition
        .relationships
        .iter()
        .filter(|relationship| relationship.relationship_type.starts_with("one"))
        .flat_map(|relationship| relationship.key_pairs.iter().map(|(field, _)| field.as_str()))
        .collect();
    let attributes = if stub {
        Vec::new()
    } else {
        definition
            .fields
            .iter()
            .filter(|field| field.is_pk || foreign.contains(field.name.as_str()))
            .map(|field| ErAttribute {
                field_type: field.field_type.clone(),
                name: field.name.clone(),
                primary: field.is_pk,
                foreign: foreign.contains(field.name.as_str()),
            })
            .collect()
    };
    ErEntity {
        full_name: definition.full_name.clone(),
        short_name: definition.entity_name.clone(),
        stub,
        attributes,
    }
}

/// The relationships of `definitions` to the entities `shown`: each `one` and `one-nofk`, and a `many` only when the
/// related entity has no `one` back (the same line otherwise); auto-reverse ones never.
pub(super) fn er_links(
    facade: &moqui_core::entity::EntityFacade,
    definitions: &[EntityDefinition],
    shown: &BTreeSet<String>,
) -> Vec<ErLink> {
    let mut links = Vec::new();
    for definition in definitions {
        for relationship in &definition.relationships {
            if relationship.is_auto_reverse || !shown.contains(&relationship.related) {
                continue;
            }
            let cardinality = match relationship.relationship_type.as_str() {
                "one" => Cardinality::One,
                "one-nofk" => Cardinality::OneNoFk,
                _ => {
                    let back = facade
                        .with_entity(&relationship.related, |related, _, _| {
                            Ok(related.relationships.iter().any(|other| {
                                other.related == definition.full_name && other.relationship_type.starts_with("one")
                            }))
                        })
                        .unwrap_or(false);
                    if back && definitions.iter().any(|shown| shown.full_name == relationship.related) {
                        continue;
                    }
                    Cardinality::Many
                }
            };
            links.push(ErLink {
                from: definition.full_name.clone(),
                to: relationship.related.clone(),
                cardinality,
                title: relationship.title.clone().unwrap_or_default(),
            });
        }
    }
    links
}
