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

//! JSON Schema for one entity, or for every entity when no name is given.

use std::collections::HashMap;

use indexmap::IndexMap;

use moqui_db::model::{EntityDefinition, EntityModel, MasterDefinition, MasterDetail, Relationship};
use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::EntityFacade;

const SCHEMA: &str = "https://json-schema.org/draft/2020-12/schema";

/// JSON Schema for `names`. An empty list is every non-view entity, or every master when `master` is set.
///
/// A master request with no master name uses `default`. As Java's `RestSchemaUtil.getJsonSchema`, an entity's
/// dependent relationships (a master's details) are properties that refer to the related entity's schema in `$defs`,
/// an array for a `many`, and a field keyed to an `Enumeration` or `StatusItem` by a titled relationship lists its ids
/// in `enum`.
pub(crate) fn document(facade: &EntityFacade, names: &[String], master: bool) -> Result<Value, Error> {
    let Some(name) = names.first() else {
        return facade.with_model(|model| Ok(Builder::new(facade, model).catalog(master)));
    };
    let master_name = master.then(|| names.get(1).cloned().unwrap_or_else(|| "default".to_string()));
    // The entity may be one built at runtime (a DB view entity, `DataDocument.<id>`); what it refers to is in the model.
    let found = facade.with_entity(name, |entity, _, model| {
        Ok(Builder::new(facade, model).one_entity(entity, master_name))
    });
    match found {
        Ok(schema) => schema,
        Err(_) => Err(bad_request(format!("No entity found with name or alias {name}"))),
    }
}

/// A name in the request that does not pick one entity or master: the client's error.
fn bad_request(message: impl Into<String>) -> Error {
    crate::context::Error::of_kind(crate::context::ErrorKind::BadRequest, message).into()
}

struct Builder<'a> {
    model: &'a EntityModel,
    /// The schemas a relationship refers to, by full entity name (`full.master` for a master in the catalog).
    defs: IndexMap<String, Value>,
    /// The key of a standalone document's own entity: a relationship back to it refers to the root, `#`.
    root: Option<String>,
    enums: FieldEnums<'a>,
}

impl<'a> Builder<'a> {
    fn new(facade: &'a EntityFacade, model: &'a EntityModel) -> Self {
        Self {
            model,
            defs: IndexMap::new(),
            root: None,
            enums: FieldEnums::new(facade),
        }
    }

    fn one_entity(&mut self, entity: &EntityDefinition, master_name: Option<String>) -> Result<Value, Error> {
        let master = match master_name.as_deref() {
            Some(master_name) => Some(entity.masters.iter().find(|item| item.name == master_name).ok_or_else(
                || {
                    bad_request(format!(
                        "Master name {master_name} not valid for entity {}",
                        entity.full_name
                    ))
                },
            )?),
            None => None,
        };
        Ok(self.standalone(entity, master))
    }

    fn catalog(&mut self, master: bool) -> Value {
        let mut entities: Vec<&EntityDefinition> = self.model.entities().iter().collect();
        entities.sort_by(|left, right| left.full_name.cmp(&right.full_name));
        for entity in entities {
            if master {
                for master_def in &entity.masters {
                    let key = format!("{}.{}", entity.full_name, master_def.name);
                    self.add_def(key, entity, Some(&master_def.details), false);
                }
            } else if !entity.is_view {
                self.add_def(entity.full_name.clone(), entity, None, true);
            }
        }
        self.defs.sort_keys();
        let mut root = IndexMap::new();
        root.insert("$schema".to_string(), Value::string(SCHEMA));
        root.insert("title".to_string(), Value::string("Moqui Entity REST API"));
        root.insert("$defs".to_string(), Value::Map(std::mem::take(&mut self.defs)));
        Value::Map(root)
    }

    fn standalone(&mut self, entity: &EntityDefinition, master: Option<&MasterDefinition>) -> Value {
        let (id, title) = match master {
            Some(master) => (
                format!("{}.{}", entity.full_name, master.name),
                format!("{} ({})", entity.entity_name, master.name),
            ),
            None => (entity.full_name.clone(), entity.entity_name.clone()),
        };
        self.root = Some(id.clone());
        let mut schema = self.object_map(entity, master.map(|master| master.details.as_slice()), master.is_none());
        schema.insert("$schema".to_string(), Value::string(SCHEMA));
        schema.insert("$id".to_string(), Value::string(id));
        schema.insert("title".to_string(), Value::string(title));
        if !self.defs.is_empty() {
            schema.insert("$defs".to_string(), Value::Map(std::mem::take(&mut self.defs)));
        }
        Value::Map(schema)
    }

    /// Put `entity`'s schema in `$defs` under `key` unless one is there. The key goes in first so a relationship loop
    /// ends, as Java puts the schema in `definitionsMap` before it follows the relationships.
    fn add_def(&mut self, key: String, entity: &EntityDefinition, details: Option<&[MasterDetail]>, nest: bool) {
        if self.defs.contains_key(&key) {
            return;
        }
        self.defs.insert(key.clone(), Value::Null);
        let schema = self.object_map(entity, details, nest);
        self.defs.insert(key, Value::Map(schema));
    }

    /// The fields, then the master's `details` when given, else the dependent relationships when `nest` is set.
    fn object_map(
        &mut self,
        entity: &EntityDefinition,
        details: Option<&[MasterDetail]>,
        nest: bool,
    ) -> IndexMap<String, Value> {
        let mut properties = IndexMap::new();
        for field in &entity.fields {
            let mut schema = field_schema(&field.field_type);
            if let Some(ids) = self.enums.of(entity, &field.name) {
                schema.insert("enum".to_string(), Value::List(ids));
            }
            properties.insert(field.name.clone(), Value::Map(schema));
        }
        if let Some(details) = details {
            for detail in details {
                let Some(relationship) = entity.relationship(&detail.relationship) else {
                    tracing::error!(
                        "Master detail relationship {} not found on {}, leaving it out of the schema",
                        detail.relationship,
                        entity.full_name
                    );
                    continue;
                };
                let combined = self.combined_details(relationship, detail);
                self.add_relationship(&mut properties, relationship, Some(&combined), false);
            }
        } else if nest {
            let model = self.model;
            for relationship in entity
                .relationships
                .iter()
                .filter(|relationship| super::info::is_dependent(model, entity, relationship))
            {
                self.add_relationship(&mut properties, relationship, None, true);
            }
        }
        let required: Vec<Value> = entity.pk_field_names().into_iter().map(Value::string).collect();
        let mut map = IndexMap::new();
        map.insert("type".to_string(), Value::string("object"));
        map.insert("properties".to_string(), Value::Map(properties));
        if !required.is_empty() {
            map.insert("required".to_string(), Value::List(required));
        }
        map
    }

    /// A detail's own details and, with `use-master`, the related entity's master's, as Java's `getDetailList`.
    fn combined_details(&self, relationship: &Relationship, detail: &MasterDetail) -> Vec<MasterDetail> {
        let mut combined = detail.details.clone();
        if let Some(name) = &detail.use_master {
            match self
                .model
                .entity_full(&relationship.related)
                .and_then(|related| related.masters.iter().find(|master| &master.name == name))
            {
                Some(master) => combined.extend(master.details.iter().cloned()),
                None => tracing::error!(
                    "Invalid use-master value [{name}], master not found on {}, leaving it out of the schema",
                    relationship.related
                ),
            }
        }
        combined
    }

    /// The property for `relationship`, named by its short alias or name: a `$ref` to the related schema, in an array
    /// for a `many`.
    fn add_relationship(
        &mut self,
        properties: &mut IndexMap<String, Value>,
        relationship: &Relationship,
        details: Option<&[MasterDetail]>,
        nest: bool,
    ) {
        let Some(related) = self.model.entity_full(&relationship.related) else {
            return;
        };
        let key = related.full_name.clone();
        let target = if self.root.as_deref() == Some(key.as_str()) {
            "#".to_string()
        } else {
            let target = format!("#/$defs/{key}");
            self.add_def(key, related, details, nest);
            target
        };
        let mut reference = IndexMap::new();
        reference.insert("$ref".to_string(), Value::string(target));
        let property = if relationship.relationship_type == "many" {
            let mut array = IndexMap::new();
            array.insert("type".to_string(), Value::string("array"));
            array.insert("items".to_string(), Value::Map(reference));
            array
        } else {
            reference
        };
        let entry = relationship
            .short_alias
            .clone()
            .filter(|alias| !alias.is_empty())
            .unwrap_or_else(|| relationship.name.clone());
        properties.insert(entry, Value::Map(property));
    }
}

/// Java's `getFieldEnums` for one document: the ids of each `(related entity, type)` are read once.
pub(crate) struct FieldEnums<'a> {
    facade: &'a EntityFacade,
    read: HashMap<(String, String), Option<Vec<Value>>>,
}

impl<'a> FieldEnums<'a> {
    pub(crate) fn new(facade: &'a EntityFacade) -> Self {
        Self {
            facade,
            read: HashMap::new(),
        }
    }

    /// Java's `getFieldEnums`: the first `one` relationship (not an auto-reverse) keyed by this field alone, when it
    /// has a title and goes to `Enumeration` or `StatusItem`, gives the ids of that type, by `sequenceNum`. A read
    /// that fails is logged and the field has no `enum`.
    pub(crate) fn of(&mut self, entity: &EntityDefinition, field_name: &str) -> Option<Vec<Value>> {
        let relationship = entity.relationships.iter().find(|relationship| {
            relationship.key_pairs.len() == 1
                && relationship.key_pairs[0].0 == field_name
                && relationship.relationship_type == "one"
                && !relationship.is_auto_reverse
        })?;
        let title = relationship.title.as_deref().filter(|title| !title.is_empty())?;
        let (type_field, id_field) = match relationship.related.as_str() {
            "moqui.basic.Enumeration" => ("enumTypeId", "enumId"),
            "moqui.basic.StatusItem" => ("statusTypeId", "statusId"),
            _ => return None,
        };
        let key = (relationship.related.clone(), title.to_string());
        if let Some(ids) = self.read.get(&key) {
            return ids.clone();
        }
        let found = self.facade.find(&relationship.related).and_then(|find| {
            find.eq(type_field, Value::string(title))
                .order_by("sequenceNum", false)
                .order_by(id_field, false)
                .disable_authz()
                .list()
        });
        let ids = match found {
            Ok(list) => {
                let ids: Vec<Value> = list
                    .values()
                    .iter()
                    .filter_map(|value| value.get(id_field).cloned())
                    .collect();
                (!ids.is_empty()).then_some(ids)
            }
            Err(err) => {
                tracing::error!(error = %err, "Could not read the {title} ids of {} for the schema", relationship.related);
                None
            }
        };
        self.read.insert(key, ids.clone());
        ids
    }
}

fn field_schema(field_type: &str) -> IndexMap<String, Value> {
    let json_type = match field_type {
        "number-integer" | "number-float" | "number-decimal" | "currency-amount" | "currency-precise" => "number",
        _ => "string",
    };
    let mut map = IndexMap::new();
    map.insert("type".to_string(), Value::string(json_type));
    let format = match field_type {
        "date-time" => Some("date-time"),
        "date" => Some("date"),
        "number-integer" => Some("int64"),
        "number-float" => Some("double"),
        _ => None,
    };
    if let Some(format) = format {
        map.insert("format".to_string(), Value::string(format));
    }
    map
}

#[cfg(test)]
mod tests {
    use moqui_lib::object::Value;

    fn at<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
        path.iter().fold(value, |current, key| match current {
            Value::Map(map) => map.get(*key).unwrap_or_else(|| panic!("no {key} in {current:?}")),
            other => panic!("not a map at {key}: {other:?}"),
        })
    }

    #[test]
    fn dependents_refer_to_their_schema_and_a_titled_enumeration_lists_its_ids() {
        let facade = crate::testing::open();
        for (id, sequence) in [("DTFDTP_MAN_PULL", 2), ("DTFDTP_RT_PUSH", 1)] {
            let mut row = facade.make_value("moqui.basic.Enumeration").unwrap();
            row.set("enumId", Value::string(id));
            row.set("enumTypeId", Value::string("DataFeedType"));
            row.set("sequenceNum", Value::I64(sequence));
            row.create(&facade).unwrap();
        }
        let feed = super::document(&facade, &["moqui.entity.feed.DataFeed".to_string()], false).unwrap();
        let properties = at(&feed, &["properties"]);
        assert_eq!(
            at(properties, &["dataFeedTypeEnumId", "enum"]),
            &Value::List(vec![Value::string("DTFDTP_RT_PUSH"), Value::string("DTFDTP_MAN_PULL")])
        );
        // A `many` dependent is an array of the related schema, which `$defs` holds.
        assert_eq!(at(properties, &["documents", "type"]), &Value::string("array"));
        assert_eq!(
            at(properties, &["documents", "items", "$ref"]),
            &Value::string("#/$defs/moqui.entity.feed.DataFeedDocument")
        );
        assert!(matches!(
            at(
                &feed,
                &[
                    "$defs",
                    "moqui.entity.feed.DataFeedDocument",
                    "properties",
                    "dataDocumentId"
                ]
            ),
            Value::Map(_)
        ));
        // The type relationship is not a dependent, so it is no property.
        let Value::Map(fields) = properties else { unreachable!() };
        assert!(!fields.contains_key("type"));

        // A dependent back to the entity itself refers to the root.
        let enumeration = super::document(&facade, &["moqui.basic.Enumeration".to_string()], false).unwrap();
        assert_eq!(at(&enumeration, &["properties", "parent", "$ref"]), &Value::string("#"));

        // The catalog refers to the schemas it holds.
        let catalog = super::document(&facade, &[], false).unwrap();
        assert_eq!(
            at(
                &catalog,
                &[
                    "$defs",
                    "moqui.entity.feed.DataFeed",
                    "properties",
                    "documents",
                    "items",
                    "$ref"
                ]
            ),
            &Value::string("#/$defs/moqui.entity.feed.DataFeedDocument")
        );
    }

    #[test]
    fn a_master_detail_refers_to_the_related_schema() {
        let facade = crate::testing::open();
        let master = super::document(
            &facade,
            &["moqui.basic.Enumeration".to_string(), "default".to_string()],
            true,
        )
        .unwrap();
        assert_eq!(
            at(&master, &["properties", "type", "$ref"]),
            &Value::string("#/$defs/moqui.basic.EnumerationType")
        );
        // The root is the master; the plain entity a detail reaches is a schema of its own.
        assert_eq!(
            at(&master, &["properties", "parent", "$ref"]),
            &Value::string("#/$defs/moqui.basic.Enumeration")
        );
        assert!(matches!(
            at(
                &master,
                &["$defs", "moqui.basic.EnumerationType", "properties", "enumTypeId"]
            ),
            Value::Map(_)
        ));
        assert!(matches!(
            at(&master, &["$defs", "moqui.basic.Enumeration", "properties", "enumId"]),
            Value::Map(_)
        ));
    }
}
