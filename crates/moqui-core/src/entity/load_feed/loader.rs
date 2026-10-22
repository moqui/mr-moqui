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

//! The entity data loader: locations and directories, records and their nested records, and the store of each
//! record in its own transaction.

use std::collections::{BTreeMap, HashMap};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use moqui_db::model::Relationship;
use moqui_lib::error::Error;
use moqui_lib::node::MNode;
use moqui_lib::object::{basic_convert, Value};

use super::csv::load_csv_reader;
use super::json::load_json_reader;
use super::xml::{load_xml_reader, load_xml_text};
use super::zip::load_zip_reader;
use super::{with_load_slot, DataFileService, LoadOptions, LoadResult, RESTRICTED_ENTITIES};
use crate::entity::facade::EntityFacade;
use crate::entity::fk::FkScope;
use crate::entity::value::EntityValue;
use crate::transaction::TransactionAttribute;

pub(super) struct Record {
    pub(super) entity: String,
    pub(super) fields: BTreeMap<String, Value>,
    pub(super) children: Vec<Record>,
}

pub(super) struct Shape {
    pub(super) full_name: String,
    pub(super) fields: HashMap<String, String>,
    pub(super) pk_names: Vec<String>,
    pub(super) relations: HashMap<String, RelInfo>,
}

pub(super) struct RelInfo {
    pub(super) related: String,
    pub(super) pairs: Vec<(String, String)>,
}

/// Load one XML, CSV, JSON, or zip location, or a directory of those files.
pub fn load_entity_data(facade: &EntityFacade, location: &str, options: &LoadOptions) -> Result<LoadResult, Error> {
    if !component_allowed(location, &options.components) {
        return Ok(LoadResult { records: 0 });
    }
    let _scope = FkScope::enter(options.dummy_fks, options.disable_fk_create);
    let path = resolve_location(facade, location)?;
    let records = crate::deterministic::traced("data", "load", location, || {
        // Java's loadSingleFile commits each file in one transaction; deterministic runs match its commits.
        if crate::deterministic::enabled() && !path.is_dir() {
            facade
                .default_transactions()?
                .run(TransactionAttribute::UseOrBegin, options.timeout, || load_path(facade, location, &path, options))
        } else {
            load_path(facade, location, &path, options)
        }
    })?;
    Ok(LoadResult { records })
}

/// Load entity XML text, as `EntityDataLoader.xmlText`. `location` names the text in errors.
pub fn load_entity_xml_text(
    facade: &EntityFacade,
    location: &str,
    text: &str,
    options: &LoadOptions,
) -> Result<LoadResult, Error> {
    let _scope = FkScope::enter(options.dummy_fks, options.disable_fk_create);
    let records = crate::deterministic::traced("data", "load", location, || {
        load_xml_text(facade, location, text, options)
    })?;
    Ok(LoadResult { records })
}

fn load_path(facade: &EntityFacade, location: &str, path: &Path, options: &LoadOptions) -> Result<u64, Error> {
    if path.is_dir() {
        return load_directory(facade, path, options);
    }
    match extension(path).as_str() {
        "xml" => {
            let file = open_data(path, location)?;
            load_xml_reader(facade, location, BufReader::new(file), options)
        }
        "csv" => {
            let file = open_data(path, location)?;
            load_csv_reader(facade, location, file, options)
        }
        "json" => {
            let file = open_data(path, location)?;
            load_json_reader(facade, location, BufReader::new(file), options)
        }
        "zip" => {
            let file = open_data(path, location)?;
            load_zip_reader(facade, location, BufReader::new(file), options, 0)
        }
        _ => Err(Error::new(format!("unsupported data file {location}"))),
    }
}

fn load_directory(facade: &EntityFacade, path: &Path, options: &LoadOptions) -> Result<u64, Error> {
    let mut names = Vec::new();
    let read = std::fs::read_dir(path)
        .map_err(|err| Error::new(format!("could not read {}", path.display())).with_source(err))?;
    for entry in read {
        let entry = entry.map_err(|err| Error::new(format!("could not read {}", path.display())).with_source(err))?;
        let child = entry.path();
        if child.is_file() && is_data_file(&child) {
            names.push(child);
        }
    }
    names.sort();
    let mut total = 0_u64;
    for child in names {
        let location = child.display().to_string();
        let records = if crate::deterministic::enabled() {
            facade.default_transactions()?.run(TransactionAttribute::UseOrBegin, options.timeout, || {
                load_path(facade, &location, &child, options)
            })?
        } else {
            load_path(facade, &location, &child, options)?
        };
        total = total.saturating_add(records);
    }
    Ok(total)
}

pub(super) enum Top {
    Record(Record),
    Service(DataFileService),
}

pub(super) fn parse_top(facade: &EntityFacade, location: &str, node: &MNode) -> Result<Option<Top>, Error> {
    let key = element_key(&node.name());
    if entity_defined(facade, &key) {
        Ok(Some(Top::Record(parse_record(facade, &key, node)?)))
    } else if let Some(name) =
        crate::service::define::defined_name(&key).or_else(|| key.contains('#').then(|| key.clone()))
    {
        Ok(Some(Top::Service(DataFileService {
            name,
            location: location.to_string(),
            parameters: service_parameters(node),
        })))
    } else {
        let failures = crate::service::define::registration_failures(&key);
        if failures.is_empty() {
            Err(Error::new(format!(
                "{location} element {} is not an entity or a service",
                node.name()
            )))
        } else {
            Err(Error::new(format!(
                "{location} element {} is a service that did not register: {}",
                node.name(),
                failures.join("; ")
            )))
        }
    }
}

pub(super) fn parse_record(facade: &EntityFacade, entity_name: &str, node: &MNode) -> Result<Record, Error> {
    parse_record_in(facade, entity_name, node, &BTreeMap::new())
}

fn parse_record_in(
    facade: &EntityFacade,
    entity_name: &str,
    node: &MNode,
    ancestor: &BTreeMap<String, Value>,
) -> Result<Record, Error> {
    let shape = shape_of(facade, entity_name)?;
    let mut fields = BTreeMap::new();
    for (name, value) in node.attributes() {
        if !shape.fields.contains_key(&name) {
            tracing::warn!(entity = %shape.full_name, field = %name, "attribute is not a field");
            continue;
        }
        if value.is_empty() {
            continue;
        }
        let field_type = shape.fields.get(&name).map(String::as_str).unwrap_or("").to_string();
        fields.insert(name, field_text(&field_type, &value));
    }
    let mut pending = Vec::new();
    for child in node.children() {
        let key = element_key(&child.name());
        if shape.fields.contains_key(&key) {
            if let Some(text) = child.text() {
                let text = text.trim();
                if !text.is_empty() {
                    let field_type = shape.fields.get(&key).map(String::as_str).unwrap_or("");
                    fields.insert(key, field_text(field_type, text));
                }
            }
        } else {
            pending.push(child);
        }
    }
    // Field elements are in `fields` before nested records, so a later element still supplies a primary key.
    let shared = shared_pks(&shape, &fields, ancestor);
    let mut children = Vec::new();
    for child in pending {
        let key = element_key(&child.name());
        if let Some(rel) = shape.relations.get(&key) {
            let mut nested = parse_record_in(facade, &rel.related, &child, &shared)?;
            apply_key_pairs(&fields, rel, &mut nested.fields);
            inherit_pk_fields(facade, &nested.entity, &shared, &mut nested.fields)?;
            children.push(nested);
        } else if entity_defined(facade, &key) {
            let mut nested = parse_record_in(facade, &key, &child, &shared)?;
            inherit_pk_fields(facade, &nested.entity, &shared, &mut nested.fields)?;
            children.push(nested);
        } else {
            tracing::warn!(entity = %shape.full_name, element = %key, "element is not a field or relationship");
        }
    }
    Ok(Record {
        entity: shape.full_name,
        fields,
        children,
    })
}

fn service_parameters(node: &MNode) -> BTreeMap<String, String> {
    let mut parameters = BTreeMap::new();
    for (name, value) in node.attributes() {
        if !value.is_empty() {
            parameters.insert(name, value);
        }
    }
    for child in node.children() {
        if let Some(text) = child.text() {
            let text = text.trim();
            if !text.is_empty() {
                parameters.insert(element_key(&child.name()), text.to_string());
            }
        }
    }
    parameters
}

fn field_text(field_type: &str, text: &str) -> Value {
    if field_type == "binary-very-long" {
        Value::Bytes(decode_binary(text))
    } else {
        Value::string(text)
    }
}

pub(super) fn decode_binary(text: &str) -> Vec<u8> {
    STANDARD
        .decode(text.trim())
        .unwrap_or_else(|_| text.as_bytes().to_vec())
}

pub(super) fn apply_key_pairs(parent: &BTreeMap<String, Value>, rel: &RelInfo, child: &mut BTreeMap<String, Value>) {
    for (source, target) in &rel.pairs {
        if child.contains_key(target) {
            continue;
        }
        if let Some(value) = parent.get(source) {
            child.insert(target.clone(), value.clone());
        }
    }
}

/// This record's primary keys, then ancestor keys whose names are not already there.
pub(super) fn shared_pks(
    shape: &Shape,
    fields: &BTreeMap<String, Value>,
    ancestor: &BTreeMap<String, Value>,
) -> BTreeMap<String, Value> {
    let mut shared = BTreeMap::new();
    for name in &shape.pk_names {
        if let Some(value) = fields.get(name).filter(|value| !value.is_null()) {
            shared.insert(name.clone(), value.clone());
        }
    }
    for (name, value) in ancestor {
        if !value.is_null() {
            shared.entry(name.clone()).or_insert_with(|| value.clone());
        }
    }
    shared
}

/// Copy primary-key values the child did not set. A key-map value already in `fields` stays.
pub(super) fn inherit_pk_fields(
    facade: &EntityFacade,
    entity_name: &str,
    source: &BTreeMap<String, Value>,
    fields: &mut BTreeMap<String, Value>,
) -> Result<(), Error> {
    if source.is_empty() {
        return Ok(());
    }
    let names = facade.with_entity(entity_name, |entity, _, _| {
        let mut names = Vec::new();
        for name in source.keys() {
            if fields.get(name).is_some_and(|value| !value.is_null()) {
                continue;
            }
            if entity.field(name).is_some() {
                names.push(name.clone());
            }
        }
        Ok(names)
    })?;
    for name in names {
        if let Some(value) = source.get(&name).filter(|value| !value.is_null()) {
            fields.insert(name, value.clone());
        }
    }
    Ok(())
}

pub(super) fn store_record(facade: &EntityFacade, options: &LoadOptions, record: &Record) -> Result<u64, Error> {
    if super::check::checking() {
        return super::check::check_record(facade, record);
    }
    if options.restrict_sensitive_entities {
        refuse_restricted(facade, record)?;
    }
    // Inside an outer transaction a failed record marks it rollback-only and gives back its join, so the
    // owner's rollback still ends the frame.
    facade
        .transactions_for(&record.entity)?
        .run(TransactionAttribute::UseOrBegin, options.timeout, || {
            write_tree(facade, options, record, &BTreeMap::new())
        })
}

/// Refuses the record when it or any nested record, by relationship name, short alias, or entity name, resolves to
/// a [`RESTRICTED_ENTITIES`] entity. Checked before anything of the tree is written.
fn refuse_restricted(facade: &EntityFacade, record: &Record) -> Result<(), Error> {
    let full_name = canonical_name(facade, &record.entity).unwrap_or_else(|_| record.entity.clone());
    if is_restricted_entity(&full_name) {
        return Err(Error::new(format!(
            "Entity {full_name} is not allowed in this data load"
        )));
    }
    for child in &record.children {
        refuse_restricted(facade, child)?;
    }
    Ok(())
}

/// `WebUtilities.isRestrictedGenericEntity`: the simple name, case-insensitive, is in [`RESTRICTED_ENTITIES`].
pub fn is_restricted_entity(entity_name: &str) -> bool {
    let simple = entity_name
        .rsplit(['.', '#'])
        .next()
        .unwrap_or(entity_name)
        .to_ascii_lowercase();
    RESTRICTED_ENTITIES.contains(&simple.as_str())
}

fn write_tree(
    facade: &EntityFacade,
    options: &LoadOptions,
    record: &Record,
    ancestor: &BTreeMap<String, Value>,
) -> Result<u64, Error> {
    crate::deterministic::trace("data", "record", &record.entity, "requested")?;
    let mut fields = record.fields.clone();
    inherit_pk_fields(facade, &record.entity, ancestor, &mut fields)?;
    prepare_fields(facade, &record.entity, &mut fields, options.only_create)?;
    // `prepare_fields` may have assigned a sequenced primary key. Children need that value.
    let shared = stored_pks(facade, &record.entity, &fields, ancestor)?;
    let partial = options.only_create && !pk_complete(facade, &record.entity, &fields)?;
    if partial {
        tracing::info!(entity = %record.entity, "onlyCreate skipped a partial primary key");
    }
    let exists = !partial && options.only_create && row_exists(facade, &record.entity, &fields)?;
    let mut total = 0_u64;
    if !partial && !exists {
        let try_insert = options.use_try_insert && try_insert_allowed(facade, &record.entity)?;
        if try_insert {
            write_upsert(facade, &record.entity, &fields)?;
        } else if options.only_create {
            value_from(facade, &record.entity, &fields)?.create(facade)?;
        } else if options.dummy_fks {
            value_from(facade, &record.entity, &fields)?.create_or_update(facade)?;
        } else {
            // Java's plain-map loader uses the entity-auto store path, including status checks.
            crate::service::call::store_entity(facade, &record.entity, &fields)?;
        }
        total = 1;
    }
    crate::deterministic::trace(
        "data",
        "record",
        &record.entity,
        if total == 0 { "skipped" } else { "succeeded" },
    )?;
    for child in &record.children {
        total = total.saturating_add(write_tree(facade, options, child, &shared)?);
    }
    Ok(total)
}

fn stored_pks(
    facade: &EntityFacade,
    entity_name: &str,
    fields: &BTreeMap<String, Value>,
    ancestor: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, Error> {
    facade.with_entity(entity_name, |entity, _, _| {
        let mut shared = BTreeMap::new();
        for name in entity.pk_field_names() {
            if let Some(value) = fields.get(name).filter(|value| !value.is_null()) {
                shared.insert(name.to_string(), value.clone());
            }
        }
        for (name, value) in ancestor {
            if !value.is_null() {
                shared.entry(name.clone()).or_insert_with(|| value.clone());
            }
        }
        Ok(shared)
    })
}

fn prepare_fields(
    facade: &EntityFacade,
    entity_name: &str,
    fields: &mut BTreeMap<String, Value>,
    only_create: bool,
) -> Result<(), Error> {
    convert_fields(facade, entity_name, fields)?;
    // `EntityAutoServiceRunner.storeEntity`, which Java's loader uses for nested records: a primary key
    // `fromDate` that is not given is now.
    let from_date_pk = facade.with_entity(entity_name, |entity, _, _| {
        Ok(entity.pk_field_names().contains(&"fromDate"))
    })?;
    if from_date_pk && !only_create && fields.get("fromDate").is_none_or(Value::is_null) {
        // `ec.user.nowTimestamp`, so it follows the effective time.
        fields.insert("fromDate".to_string(), crate::user::now_effective_value());
    }
    let (single, missing, only_last) = facade.with_entity(entity_name, |entity, _, _| {
        let names = entity.pk_field_names();
        let absent = |name: &&str| fields.get(*name).is_none_or(Value::is_null);
        let missing = names.iter().any(absent);
        // Every key but the last given: the last is a secondary sequence (`setSequencedIdSecondary`).
        let only_last = names.len() > 1
            && names.last().is_some_and(absent)
            && names[..names.len() - 1].iter().all(|name| !absent(name));
        Ok((names.len() == 1, missing, only_last))
    })?;
    if only_last && !only_create {
        let mut value = value_from(facade, entity_name, fields)?;
        value.set_sequenced_id_secondary(facade)?;
        let pk_name = facade.with_entity(entity_name, |entity, _, _| {
            Ok(entity.pk_field_names().last().copied().unwrap_or("").to_string())
        })?;
        if let Some(value) = value.get(&pk_name).cloned() {
            fields.insert(pk_name, value);
        }
    }
    if missing && single && !only_create {
        let mut value = value_from(facade, entity_name, fields)?;
        value.set_sequenced_id_primary(facade)?;
        let pk_name = facade.with_entity(entity_name, |entity, _, _| {
            Ok(entity.pk_field_names().first().copied().unwrap_or("").to_string())
        })?;
        if let Some(value) = value.get(&pk_name).cloned() {
            fields.insert(pk_name, value);
        }
    }
    Ok(())
}

pub(super) fn convert_fields(
    facade: &EntityFacade,
    entity_name: &str,
    fields: &mut BTreeMap<String, Value>,
) -> Result<(), Error> {
    let updates = facade.with_entity(entity_name, |entity, _, model| {
        let mut updates = BTreeMap::new();
        for field in &entity.fields {
            let Some(value) = fields.get(&field.name) else {
                continue;
            };
            if value.is_null() {
                continue;
            }
            let Some(java) = model
                .dictionary_type(&field.field_type)
                .map(|entry| entry.java_type.clone())
            else {
                continue;
            };
            let mut converted = basic_convert(value, &java);
            // Java parses number text with the locale (`L10nFacade.parseNumber`), so `118,500` is 118500.
            if let Value::String(text) = value {
                if !converted.changed && text.contains(',') && is_number_type(&java) {
                    let plain = Value::String(text.replace(',', ""));
                    let retried = basic_convert(&plain, &java);
                    if retried.changed {
                        converted = retried;
                    }
                }
            }
            updates.insert(field.name.clone(), converted.value);
        }
        Ok(updates)
    })?;
    fields.extend(updates);
    Ok(())
}

fn is_number_type(java_type: &str) -> bool {
    matches!(
        java_type.rsplit('.').next().unwrap_or(java_type),
        "BigDecimal" | "BigInteger" | "Long" | "Integer" | "Double" | "Float" | "long" | "int" | "double" | "float"
    )
}

fn pk_complete(facade: &EntityFacade, entity_name: &str, fields: &BTreeMap<String, Value>) -> Result<bool, Error> {
    facade.with_entity(entity_name, |entity, _, _| {
        Ok(entity
            .pk_field_names()
            .iter()
            .all(|name| fields.get(*name).is_some_and(|value| !value.is_null())))
    })
}

fn row_exists(facade: &EntityFacade, entity_name: &str, fields: &BTreeMap<String, Value>) -> Result<bool, Error> {
    let keys = facade.with_entity(entity_name, |entity, _, _| {
        let mut keys = BTreeMap::new();
        for name in entity.pk_field_names() {
            let value = fields
                .get(name)
                .filter(|value| !value.is_null())
                .cloned()
                .ok_or_else(|| Error::new(format!("primary key {name} is required on {}", entity.full_name)))?;
            keys.insert(name.to_string(), value);
        }
        Ok(keys)
    })?;
    let mut find = facade.find(entity_name)?;
    for (field, value) in keys {
        find = find.eq(&field, value);
    }
    Ok(find.one()?.is_some())
}

fn try_insert_allowed(facade: &EntityFacade, entity_name: &str) -> Result<bool, Error> {
    facade.with_entity(entity_name, |_, dialect, _| Ok(!dialect.never_try_insert))
}

fn write_upsert(facade: &EntityFacade, entity_name: &str, fields: &BTreeMap<String, Value>) -> Result<(), Error> {
    facade.upsert_record(entity_name, fields)
}

fn value_from(
    facade: &EntityFacade,
    entity_name: &str,
    fields: &BTreeMap<String, Value>,
) -> Result<EntityValue, Error> {
    let mut value = EntityValue::make(facade, entity_name)?;
    for (name, field) in fields {
        value.set(name, field.clone());
    }
    Ok(value)
}

pub(super) fn call_data_file_service(
    facade: &EntityFacade,
    options: &LoadOptions,
    service: DataFileService,
) -> Result<(), Error> {
    if super::check::checking() {
        super::check::skip_service(&service);
        return Ok(());
    }
    // A restricted load takes records only: a service element could write anything.
    if options.restrict_sensitive_entities {
        tracing::warn!(
            "Refusing service {} from {}: restricted data loads take records only",
            service.name,
            service.location
        );
        return Err(Error::new(format!(
            "Service {} is not allowed in this data load",
            service.name
        )));
    }
    with_load_slot(facade, |slot| slot.services.push(service.clone()));
    crate::service::run_data_file_service(facade, &service)
}

pub(super) fn shape_of(facade: &EntityFacade, entity_name: &str) -> Result<Shape, Error> {
    facade.with_entity(entity_name, |entity, _, model| {
        let mut fields = HashMap::new();
        for field in &entity.fields {
            fields.insert(field.name.clone(), field.field_type.clone());
        }
        let mut relations = HashMap::new();
        for relationship in &entity.relationships {
            let related = model
                .entity(&relationship.related)
                .map(|related| related.full_name.clone())
                .unwrap_or_else(|_| relationship.related.clone());
            let info = RelInfo {
                related,
                pairs: key_pairs(relationship),
            };
            for key in relation_keys(relationship) {
                relations.entry(key).or_insert_with(|| RelInfo {
                    related: info.related.clone(),
                    pairs: info.pairs.clone(),
                });
            }
        }
        let pk_names = entity.pk_field_names().into_iter().map(str::to_string).collect();
        Ok(Shape {
            full_name: entity.full_name.clone(),
            fields,
            pk_names,
            relations,
        })
    })
}

fn key_pairs(relationship: &Relationship) -> Vec<(String, String)> {
    relationship
        .key_map
        .iter()
        .map(|pair| {
            let target = pair.related.clone().unwrap_or_else(|| pair.field_name.clone());
            (pair.field_name.clone(), target)
        })
        .collect()
}

fn relation_keys(relationship: &Relationship) -> Vec<String> {
    let mut keys = vec![relationship.name.clone()];
    if let Some(alias) = &relationship.short_alias {
        if !keys.iter().any(|key| key == alias) {
            keys.push(alias.clone());
        }
    }
    if relationship.title.is_none() {
        for name in [&relationship.related_entity_name, &relationship.related] {
            if !keys.iter().any(|key| key == name) {
                keys.push(name.clone());
            }
        }
    }
    keys
}

pub(super) fn entity_defined(facade: &EntityFacade, name: &str) -> bool {
    facade.with_entity(name, |_, _, _| Ok(())).is_ok()
}

pub(super) fn canonical_name(facade: &EntityFacade, name: &str) -> Result<String, Error> {
    facade.with_entity(name, |entity, _, _| Ok(entity.full_name.clone()))
}

fn element_key(name: &str) -> String {
    let rest = name.rsplit(':').next().unwrap_or(name);
    rest.replace('-', "#")
}

pub(super) fn skip_type(file_type: Option<&str>, data_types: &[String]) -> bool {
    match file_type {
        Some(file_type) if !data_types.is_empty() => !data_types.iter().any(|allowed| allowed == file_type),
        _ => false,
    }
}

fn component_allowed(location: &str, components: &[String]) -> bool {
    if components.is_empty() {
        return true;
    }
    match component_name(location) {
        Some(name) => components.iter().any(|allowed| allowed == name),
        None => true,
    }
}

fn component_name(location: &str) -> Option<&str> {
    let rest = location
        .strip_prefix("component://")
        .or_else(|| location.strip_prefix("component:"))?;
    let rest = rest.trim_start_matches('/');
    if rest.is_empty() {
        return None;
    }
    rest.split('/').next()
}

/// `classpath://` is a compatibility alias for a path under the framework directory (`service/`,
/// `entity/`, `data/`, `screen/`, `template/`). Other schemes use the entered factory's components.
fn resolve_location(facade: &EntityFacade, location: &str) -> Result<PathBuf, Error> {
    for prefix in ["classpath://", "classpath:"] {
        if let Some(rest) = location.strip_prefix(prefix) {
            return Ok(facade.framework_dir().join(rest.trim_start_matches('/')));
        }
    }
    crate::context::locate(location).map_err(|err| Error::new(err.message().to_string()))
}

pub(super) fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn is_data_file(path: &Path) -> bool {
    matches!(extension(path).as_str(), "xml" | "csv" | "json" | "zip")
}

fn open_data(path: &Path, location: &str) -> Result<std::fs::File, Error> {
    std::fs::File::open(path).map_err(|err| Error::new(format!("could not read {location}")).with_source(err))
}
