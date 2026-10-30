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

//! `entities.d.ts`: one shape for each entity and view entity (field types, primary key, relationships), and the map
//! from every name the definition cache answers to (full name, unique entity name, short alias) to that shape.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

use moqui_db::model::{EntityDefinition, EntityModel};

use super::ts::{doc, ident, quote, server_type};

/// The interface name of each entity in `MoquiEntity`, by full name. Two full names that make the same identifier get a
/// numbered suffix.
pub(super) fn entity_idents(model: &EntityModel) -> BTreeMap<String, String> {
    let mut used = BTreeSet::new();
    let mut out = BTreeMap::new();
    let mut names: Vec<&str> = model.full_names();
    names.sort_unstable();
    for full_name in names {
        let base = ident(full_name);
        let mut name = base.clone();
        let mut index = 2;
        while !used.insert(name.clone()) {
            name = format!("{base}_{index}");
            index += 1;
        }
        out.insert(full_name.to_string(), name);
    }
    out
}

pub(super) fn emit(model: &EntityModel) -> String {
    let idents = entity_idents(model);
    let mut by_full: Vec<&EntityDefinition> = model.entities().iter().collect();
    by_full.sort_by(|left, right| left.full_name.cmp(&right.full_name));
    let mut out = String::from(super::HEADER);
    out.push_str(
        "// Entity and view-entity definitions. Field types are what the host's `get` returns on the server.\n\n",
    );
    out.push_str("declare namespace MoquiEntity {\n");
    for entity in &by_full {
        write_entity(&mut out, entity, &idents[&entity.full_name]);
    }
    out.push_str("}\n\n");

    // Every name `EntityModel::entity` resolves, as Java's definition cache keys them.
    let mut short_count: HashMap<&str, usize> = HashMap::new();
    for entity in &by_full {
        *short_count.entry(entity.entity_name.as_str()).or_default() += 1;
    }
    let mut keys: BTreeMap<String, &str> = BTreeMap::new();
    for entity in &by_full {
        keys.insert(entity.full_name.clone(), &entity.full_name);
    }
    for entity in &by_full {
        if short_count.get(entity.entity_name.as_str()) == Some(&1) {
            keys.entry(entity.entity_name.clone()).or_insert(&entity.full_name);
        }
    }
    for entity in &by_full {
        if let Some(alias) = entity.short_alias.as_ref().filter(|alias| !alias.is_empty()) {
            keys.entry(alias.clone()).or_insert(&entity.full_name);
        }
    }
    out.push_str(
        "/** Entity shapes by every name the entity facade resolves: full name, unique entity name, short alias. */\n",
    );
    out.push_str("interface MoquiEntityMap {\n");
    for (key, full_name) in &keys {
        let _ = writeln!(out, "    {}: MoquiEntity.{};", quote(key), idents[*full_name]);
    }
    out.push_str("}\n");
    out
}

fn write_entity(out: &mut String, entity: &EntityDefinition, name: &str) {
    let pk = entity.pk_field_names();
    let kind = if entity.is_view { "view-entity" } else { "entity" };
    let pk_text = if pk.is_empty() {
        "none".to_string()
    } else {
        pk.join(", ")
    };
    let _ = writeln!(
        out,
        "    /** {} ({kind}). Primary key: {}. */",
        doc(&entity.full_name),
        doc(&pk_text)
    );
    let _ = writeln!(out, "    interface {name} {{");
    out.push_str("        fields: {\n");
    for field in entity.field_names() {
        let java = entity.java_type_of(field).unwrap_or_default();
        let _ = writeln!(out, "            {}: {};", quote(field), server_type(java));
    }
    out.push_str("        };\n");
    let pk_type = if pk.is_empty() {
        "never".to_string()
    } else {
        pk.iter().map(|name| quote(name)).collect::<Vec<_>>().join(" | ")
    };
    let _ = writeln!(out, "        pk: {pk_type};");
    // The names `EntityDefinition::relationship` matches, each to the related entity's full name.
    let mut relationships: BTreeMap<String, &str> = BTreeMap::new();
    for relationship in &entity.relationships {
        let related = relationship.related.as_str();
        relationships.entry(relationship.name.clone()).or_insert(related);
        if let Some(alias) = relationship.short_alias.as_ref().filter(|alias| !alias.is_empty()) {
            relationships.entry(alias.clone()).or_insert(related);
        }
        if relationship.title.is_none() {
            relationships
                .entry(relationship.related_entity_name.clone())
                .or_insert(related);
        }
    }
    if relationships.is_empty() {
        out.push_str("        rel: {};\n");
    } else {
        out.push_str("        rel: {\n");
        for (key, related) in &relationships {
            let _ = writeln!(out, "            {}: {};", quote(key), quote(related));
        }
        out.push_str("        };\n");
    }
    out.push_str("    }\n");
}
