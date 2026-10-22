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

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::alphabet;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::DecodePaddingMode;
use base64::Engine;
use moqui_db::model::Relationship;
use moqui_lib::error::Error;
use moqui_lib::node::MNode;
use moqui_lib::object::{convert_checked, is_java_type, to_plain_string, Value};

use super::csv::load_csv_reader;
use super::json::load_json_reader;
use super::xml::{load_xml_reader, load_xml_text};
use super::zip::load_zip_reader;
use super::{error_chain, with_load_slot, DataFileService, LoadOptions, LoadResult};
use crate::entity::FieldMap;
use crate::entity::facade::EntityFacade;
use crate::entity::fk::FkScope;
use crate::entity::value::EntityValue;
use crate::transaction::TransactionAttribute;

pub(super) struct Record {
    pub(super) entity: String,
    pub(super) fields: FieldMap,
    pub(super) children: Vec<Record>,
}

pub(super) struct Shape {
    pub(super) full_name: String,
    /// Each field's type, by the field's name: the one name the entity's values share ([`EntityValue::set`]).
    pub(super) fields: HashMap<Arc<str>, String>,
    /// Shared, as [`Self::fields`]'s names are.
    pub(super) pk_names: Vec<Arc<str>>,
    pub(super) relations: HashMap<String, RelInfo>,
    /// Each field with a dictionary type, in definition order, and that type's Java type.
    java_types: Vec<(String, String)>,
}

impl Shape {
    /// The shared name of field `name`, or a name of its own for one the entity does not have.
    pub(super) fn name(&self, name: &str) -> Arc<str> {
        self.fields
            .get_key_value(name)
            .map_or_else(|| Arc::from(name), |(shared, _)| Arc::clone(shared))
    }
}

pub(super) struct RelInfo {
    pub(super) related: String,
    pub(super) pairs: Vec<(String, String)>,
}

/// Load one XML, CSV, JSON, or zip location, or a directory of those files. As Java, `options.components` does not
/// filter a location given here; the callers that gather locations use [`location_in_components`].
pub fn load_entity_data(facade: &EntityFacade, location: &str, options: &LoadOptions) -> Result<LoadResult, Error> {
    let _scope = FkScope::enter(options.dummy_fks, options.disable_fk_create);
    let path = resolve_location(facade, location)?;
    let mut failed_files = 0;
    let records = load_path(facade, location, &path, options, &mut failed_files)?;
    Ok(LoadResult { records, failed_files })
}

/// Load entity XML text, as `EntityDataLoader.xmlText`. `location` names the text in errors.
pub fn load_entity_xml_text(
    facade: &EntityFacade,
    location: &str,
    text: &str,
    options: &LoadOptions,
) -> Result<LoadResult, Error> {
    let _scope = FkScope::enter(options.dummy_fks, options.disable_fk_create);
    let records = super::check::at_location(location, || {
        in_file_transaction(facade, options, || load_xml_text(facade, location, text, options))
    })?;
    Ok(LoadResult {
        records,
        failed_files: 0,
    })
}

/// CSV or JSON text written to the file `path`, as Java's `csvText` and `jsonText`: one transaction, named `label`,
/// and in a check no `-- Checked data` message.
pub fn load_entity_text_file(
    facade: &EntityFacade,
    label: &str,
    path: &Path,
    options: &LoadOptions,
) -> Result<LoadResult, Error> {
    let _scope = FkScope::enter(options.dummy_fks, options.disable_fk_create);
    let records = super::check::at_location(label, || {
        in_file_transaction(facade, options, || load_file(facade, label, path, options))
    })?;
    Ok(LoadResult {
        records,
        failed_files: 0,
    })
}

fn load_path(
    facade: &EntityFacade,
    location: &str,
    path: &Path,
    options: &LoadOptions,
    failed_files: &mut u64,
) -> Result<u64, Error> {
    if path.is_dir() {
        return load_directory(facade, path, options, failed_files);
    }
    super::check::check_file(location, options, || {
        in_file_transaction(facade, options, || load_file(facade, location, path, options))
    })
}

/// Java's `loadSingleFile`: one file (or text) in one transaction, 600 seconds unless the load names another timeout.
/// A record that fails rolls back the whole file, and the load reports it; inside a caller's transaction the file
/// joins it.
pub(super) fn in_file_transaction(
    facade: &EntityFacade,
    options: &LoadOptions,
    body: impl FnOnce() -> Result<u64, Error>,
) -> Result<u64, Error> {
    if super::check::checking() {
        return body();
    }
    let timeout = options.timeout.unwrap_or(FILE_TIMEOUT);
    let tx = facade.default_transactions()?;
    tx.run(TransactionAttribute::UseOrBegin, Some(timeout), || {
        // The file's writes go without waiting for their results ...
        let _pipeline = crate::entity::exec::PipelineWrites::enter();
        let loaded = body()?;
        // ... which are all in before the file's transaction ends, or the caller's goes on: a failure fails the file.
        tx.finish_deferred()?;
        Ok(loaded)
    })
}

/// Java's `EntityDataLoaderImpl.transactionTimeout` default.
const FILE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

fn load_file(facade: &EntityFacade, location: &str, path: &Path, options: &LoadOptions) -> Result<u64, Error> {
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

/// Each file of the directory, by name. As in Java's `loadAllData`, a file that fails is logged and the load goes
/// on with the next file; the failure is counted in `failed_files`.
fn load_directory(
    facade: &EntityFacade,
    path: &Path,
    options: &LoadOptions,
    failed_files: &mut u64,
) -> Result<u64, Error> {
    let mut names = Vec::new();
    let read = std::fs::read_dir(path)
        .map_err(|err| Error::new(format!("could not read {}", path.display())).with_source(err))?;
    for entry in read {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                tracing::error!(error = %err, "Skipping an entry of {} that could not be read", path.display());
                *failed_files += 1;
                continue;
            }
        };
        let child = entry.path();
        if child.is_file() && is_data_file(&child) {
            names.push(child);
        }
    }
    names.sort();
    let mut total = 0_u64;
    for child in names {
        let location = child.display().to_string();
        match load_path(facade, &location, &child, options, failed_files) {
            Ok(records) => total = total.saturating_add(records),
            Err(err) => {
                tracing::error!(
                    "Skipping to next file after error loading {location}: {}",
                    error_chain(&err)
                );
                *failed_files += 1;
            }
        }
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
    } else if let Some(name) = data_service_name(&key) {
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

/// The service a data file names, as Java's `isServiceDefined` takes it, or a `verb#noun` entity-auto name.
pub(super) fn data_service_name(name: &str) -> Option<String> {
    crate::service::define::defined_name(name).or_else(|| name.contains('#').then(|| name.to_string()))
}

/// A top-level element read without a node: one with no child elements ([`MNode::parse_flat`]), the usual record of a
/// data file. `None` for any other, which [`parse_top`] reads from its node.
pub(super) fn parse_flat_top(facade: &EntityFacade, location: &str, text: &str) -> Result<Option<Top>, Error> {
    let Some(element) = MNode::parse_flat(location, text)? else {
        return Ok(None);
    };
    let key = element_key(element.name);
    if !entity_defined(facade, &key) {
        return Ok(None);
    }
    let shape = shape_of(facade, &key)?;
    let mut fields = BTreeMap::new();
    for (name, value) in element.attributes {
        add_attribute(&shape, &mut fields, name, value);
    }
    Ok(Some(Top::Record(Record {
        entity: shape.full_name.clone(),
        fields,
        children: Vec::new(),
    })))
}

/// An attribute of a record element, as its field's value. Java puts an empty attribute as null, so a store clears
/// the column.
fn add_attribute(shape: &Shape, fields: &mut FieldMap, name: &str, value: Cow<'_, str>) {
    let Some(field_type) = shape.fields.get(name) else {
        tracing::warn!(entity = %shape.full_name, field = %name, "attribute is not a field");
        return;
    };
    let value = if value.is_empty() {
        Value::Null
    } else if field_type == "binary-very-long" {
        Value::Bytes(decode_binary(&value))
    } else {
        Value::String(value.into_owned())
    };
    fields.insert(shape.name(name), value);
}

pub(super) fn parse_record(facade: &EntityFacade, entity_name: &str, node: &MNode) -> Result<Record, Error> {
    parse_record_in(facade, entity_name, node, &BTreeMap::new())
}

fn parse_record_in(
    facade: &EntityFacade,
    entity_name: &str,
    node: &MNode,
    ancestor: &FieldMap,
) -> Result<Record, Error> {
    let shape = shape_of(facade, entity_name)?;
    let mut fields = BTreeMap::new();
    node.for_each_attribute(|name, value| add_attribute(&shape, &mut fields, name, Cow::Borrowed(value)));
    let mut pending = Vec::new();
    for child in node.children() {
        let key = element_key(&child.name());
        if shape.fields.contains_key(key.as_str()) {
            // The text as written, whitespace included, as Java's SAX handler reads it (the XML reader parses raw).
            if let Some(text) = child.text().filter(|text| !text.is_empty()) {
                let field_type = shape.fields.get(key.as_str()).map(String::as_str).unwrap_or("");
                fields.insert(shape.name(&key), field_text(field_type, &text));
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
        entity: shape.full_name.clone(),
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
        if let Some(text) = child.text().filter(|text| !text.is_empty()) {
            parameters.insert(element_key(&child.name()), text);
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

/// A `binary-very-long` value: Base64, or the UTF-8 bytes of text that is not, as Java's XML reader. The text is
/// trimmed first, so Base64 laid out on lines of its own still decodes, where Java's strict decoder takes it as text.
pub(super) fn decode_binary(text: &str) -> Vec<u8> {
    decode_base64(text.trim()).unwrap_or_else(|| text.as_bytes().to_vec())
}

/// Java's `Base64.getDecoder()`: the standard alphabet, with the padding optional.
pub(super) fn decode_base64(text: &str) -> Option<Vec<u8>> {
    const JAVA_BASE64: GeneralPurpose = GeneralPurpose::new(
        &alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
    );
    JAVA_BASE64.decode(text).ok()
}

pub(super) fn apply_key_pairs(parent: &FieldMap, rel: &RelInfo, child: &mut FieldMap) {
    for (source, target) in &rel.pairs {
        if child.get(target.as_str()).is_some_and(|value| !value.is_null()) {
            continue;
        }
        if let Some(value) = parent.get(source.as_str()) {
            child.insert(Arc::from(target.as_str()), value.clone());
        }
    }
}

/// This record's primary keys, then ancestor keys whose names are not already there.
pub(super) fn shared_pks(
    shape: &Shape,
    fields: &FieldMap,
    ancestor: &FieldMap,
) -> FieldMap {
    let mut shared = BTreeMap::new();
    for name in &shape.pk_names {
        if let Some(value) = fields.get(&**name).filter(|value| !value.is_null()) {
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
    source: &FieldMap,
    fields: &mut FieldMap,
) -> Result<(), Error> {
    if source.is_empty() {
        return Ok(());
    }
    let shape = shape_of(facade, entity_name)?;
    for (name, value) in source {
        if value.is_null() || !shape.fields.contains_key(name) || fields.get(name).is_some_and(|held| !held.is_null()) {
            continue;
        }
        fields.insert(name.clone(), value.clone());
    }
    Ok(())
}

pub(super) fn store_record(facade: &EntityFacade, options: &LoadOptions, record: Record) -> Result<u64, Error> {
    if super::check::checking() {
        return super::check::check_record(facade, &record);
    }
    if options.restrict_sensitive_entities {
        refuse_restricted(facade, &record)?;
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
/// a restricted entity ([`is_restricted_entity`]). Checked before anything of the tree is written.
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

/// `WebUtilities.isRestrictedGenericEntity` of an entity name, or of the noun of a `verb#noun` service name.
pub fn is_restricted_entity(entity_name: &str) -> bool {
    let noun = entity_name.rsplit('#').next().unwrap_or(entity_name);
    crate::entity::sensitive::is_restricted_generic_entity(noun)
}

fn write_tree(
    facade: &EntityFacade,
    options: &LoadOptions,
    record: Record,
    ancestor: &FieldMap,
) -> Result<u64, Error> {
    let Record {
        entity,
        mut fields,
        children,
    } = record;
    let shape = shape_of(facade, &entity)?;
    inherit_pk_fields(facade, &entity, ancestor, &mut fields)?;
    prepare_fields(facade, &shape, &mut fields, options.only_create)?;
    // `prepare_fields` may have assigned a sequenced primary key. Children need that value.
    let shared = shared_pks(&shape, &fields, ancestor);
    let partial = options.only_create && !pk_complete(&shape, &fields);
    if partial {
        tracing::info!(entity = %entity, "onlyCreate skipped a partial primary key");
    }
    let exists = !partial && options.only_create && row_exists(facade, &shape, &fields)?;
    let mut total = 0_u64;
    if !partial && !exists {
        if options.only_create {
            value_from(facade, &entity, &fields)?.create(facade)?;
        } else if facade.upsert_fits(&entity, &fields)? {
            // One statement whether or not the row is there, which a pipelined load does not wait for.
            write_upsert(facade, &entity, fields)?;
        } else if options.dummy_fks {
            value_from(facade, &entity, &fields)?.create_or_update(facade)?;
        } else {
            // Java's plain-map loader (XML and JSON records) uses the entity-auto store path, including status checks.
            // CSV rows go through `store_value` instead.
            crate::service::call::store_entity(facade, &entity, &crate::entity::string_map(&fields))?;
        }
        total = 1;
    }
    for child in children {
        total = total.saturating_add(write_tree(facade, options, child, &shared)?);
    }
    Ok(total)
}

/// A CSV row, as Java's `handleValue`: its primary key is complete ([`complete_csv_pk`]), and it is created when
/// missing (`onlyCreate`), stored with one upsert where that fits ([`EntityFacade::upsert_fits`]), or else
/// `createOrUpdate`d. There is no entity-auto `fromDate` default, secondary sequence or status-flow check, which
/// Java's CSV path does not run either.
pub(super) fn store_value(facade: &EntityFacade, options: &LoadOptions, record: Record) -> Result<u64, Error> {
    if super::check::checking() {
        return super::check::check_record(facade, &record);
    }
    if options.restrict_sensitive_entities {
        refuse_restricted(facade, &record)?;
    }
    let Record { entity, fields, .. } = record;
    let entity = entity.as_str();
    facade
        .transactions_for(entity)?
        .run(TransactionAttribute::UseOrBegin, options.timeout, || {
            if options.only_create {
                if !row_exists(facade, &*shape_of(facade, entity)?, &fields)? {
                    value_from(facade, entity, &fields)?.create(facade)?;
                }
            } else if facade.upsert_fits(entity, &fields)? {
                write_upsert(facade, entity, fields)?;
            } else {
                value_from(facade, entity, &fields)?.create_or_update(facade)?;
            }
            Ok(1)
        })
}

/// Java's CSV primary-key check: with the fields converted to their types, a missing key of a one-field primary key
/// is sequenced, and a missing key of a longer one is an error.
pub(super) fn complete_csv_pk(
    facade: &EntityFacade,
    entity_name: &str,
    fields: &mut FieldMap,
) -> Result<(), Error> {
    convert_fields(facade, entity_name, fields)?;
    let shape = shape_of(facade, entity_name)?;
    if pk_complete(&shape, fields) {
        return Ok(());
    }
    let pk_names = &shape.pk_names;
    let [pk_name] = pk_names.as_slice() else {
        let given: Vec<String> = pk_names
            .iter()
            .map(|name| {
                let value = fields.get(&**name).map_or_else(|| "null".to_string(), to_plain_string);
                format!("{name}={value}")
            })
            .collect();
        return Err(Error::new(format!(
            "Cannot process value with incomplete primary key for [{entity_name}] with more than 1 primary key field: [{}]",
            given.join(", ")
        )));
    };
    let mut value = value_from(facade, entity_name, fields)?;
    value.set_sequenced_id_primary(facade)?;
    if let Some(value) = value.get(pk_name).cloned() {
        fields.insert(pk_name.clone(), value);
    }
    Ok(())
}

fn prepare_fields(
    facade: &EntityFacade,
    shape: &Shape,
    fields: &mut FieldMap,
    only_create: bool,
) -> Result<(), Error> {
    convert_with(shape, fields)?;
    let names = &shape.pk_names;
    // `EntityAutoServiceRunner.storeEntity`, which Java's loader uses for nested records: a primary key
    // `fromDate` that is not given is now.
    let from_date_pk = names.iter().any(|name| &**name == "fromDate");
    if from_date_pk && !only_create && fields.get("fromDate").is_none_or(Value::is_null) {
        // `ec.user.nowTimestamp`, so it follows the effective time.
        fields.insert(shape.name("fromDate"), crate::user::now_effective_value());
    }
    let absent = |name: &Arc<str>| fields.get(&**name).is_none_or(Value::is_null);
    let missing = names.iter().any(absent);
    // Every key but the last given: the last is a secondary sequence (`setSequencedIdSecondary`).
    let only_last = names.len() > 1
        && names.last().is_some_and(absent)
        && names[..names.len() - 1].iter().all(|name| !absent(name));
    let single = names.len() == 1;
    if only_last && !only_create {
        let mut value = value_from(facade, &shape.full_name, fields)?;
        value.set_sequenced_id_secondary(facade)?;
        if let Some(pk_name) = names.last() {
            if let Some(value) = value.get(pk_name).cloned() {
                fields.insert(pk_name.clone(), value);
            }
        }
    }
    if missing && single && !only_create {
        let mut value = value_from(facade, &shape.full_name, fields)?;
        value.set_sequenced_id_primary(facade)?;
        if let Some(pk_name) = names.first() {
            if let Some(value) = value.get(pk_name).cloned() {
                fields.insert(pk_name.clone(), value);
            }
        }
    }
    Ok(())
}

/// Each field's text as its type, as Java's `FieldInfo.convertFromString`. Text the type does not take fails the
/// record, as Java's validation error (XML, JSON) or exception (CSV) does.
pub(super) fn convert_fields(
    facade: &EntityFacade,
    entity_name: &str,
    fields: &mut FieldMap,
) -> Result<(), Error> {
    convert_with(&*shape_of(facade, entity_name)?, fields)
}

fn convert_with(shape: &Shape, fields: &mut FieldMap) -> Result<(), Error> {
    for (name, java) in &shape.java_types {
        let Some(value) = fields.get_mut(name.as_str()).filter(|value| !value.is_null()) else {
            continue;
        };
        // Already of the type, so `convert_text` would copy it, unless it is the text `null`.
        if is_java_type(value, java) && !matches!(value, Value::String(text) if text == "null") {
            continue;
        }
        let converted = convert_text(value, java).ok_or_else(|| {
            Error::new(format!(
                "The value [{}] is not valid for type {java} for field {}.{name}",
                to_plain_string(value),
                shape.full_name,
            ))
        })?;
        *value = converted;
    }
    Ok(())
}

/// One value as `java_type`, or `None` when it is not of that type. A timestamp without an offset is in the session
/// time zone, as Java's `convertFromString` reads it through `ec.l10n.parseTimestamp`: the loading user's, else the
/// server's (a load at start or `moqui load` has no user).
fn convert_text(value: &Value, java_type: &str) -> Option<Value> {
    if let Value::String(text) = value {
        // `convertFromString` reads the text `null` as null, for any type.
        if text == "null" {
            return Some(Value::Null);
        }
        // `Boolean.valueOf`: only `true`, in any case, is true.
        if matches!(java_type.rsplit('.').next(), Some("Boolean" | "boolean")) {
            return Some(Value::Bool(text.eq_ignore_ascii_case("true")));
        }
    }
    if let Some(converted) = crate::l10n::convert_checked(value, java_type) {
        return Some(converted);
    }
    // Java parses number text with the locale (`L10nFacade.parseNumber`) to a BigDecimal and takes the type's value
    // of that, so `118,500` is 118500 and `1800.0` is 1800 for a Long.
    match value {
        Value::String(text) if is_number_type(java_type) => {
            let decimal = convert_checked(&Value::String(text.replace(',', "")), "java.math.BigDecimal")?;
            convert_checked(&decimal, java_type)
        }
        _ => None,
    }
}

fn is_number_type(java_type: &str) -> bool {
    matches!(
        java_type.rsplit('.').next().unwrap_or(java_type),
        "BigDecimal" | "BigInteger" | "Long" | "Integer" | "Double" | "Float" | "long" | "int" | "double" | "float"
    )
}

fn pk_complete(shape: &Shape, fields: &FieldMap) -> bool {
    shape
        .pk_names
        .iter()
        .all(|name| fields.get(&**name).is_some_and(|value| !value.is_null()))
}

fn row_exists(facade: &EntityFacade, shape: &Shape, fields: &FieldMap) -> Result<bool, Error> {
    let mut find = facade.find(&shape.full_name)?;
    for name in &shape.pk_names {
        let value = fields
            .get(&**name)
            .filter(|value| !value.is_null())
            .cloned()
            .ok_or_else(|| Error::new(format!("primary key {name} is required on {}", shape.full_name)))?;
        find = find.eq(name, value);
    }
    Ok(find.one()?.is_some())
}

/// The record's fields keep the names they share with the entity's values.
fn write_upsert(facade: &EntityFacade, entity_name: &str, fields: FieldMap) -> Result<(), Error> {
    facade.upsert_record(entity_name, fields)
}

fn value_from(
    facade: &EntityFacade,
    entity_name: &str,
    fields: &FieldMap,
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
    // As Java's `handleService`: a create-only load calls no service.
    if options.only_create {
        tracing::info!(
            "Not calling service {} from {}, running with only insert",
            service.name,
            service.location
        );
        return Ok(());
    }
    with_load_slot(facade, |slot| slot.services.push(service.clone()));
    crate::service::run_data_file_service(facade, &service)
}

/// Java `defaultValues` on a top-level record: each default the record does not set, for a field of its entity. A
/// field the record sets empty (null) stays null, as Java puts the record's values over the defaults.
pub(super) fn apply_default_values(facade: &EntityFacade, options: &LoadOptions, record: &mut Record) {
    if options.default_values.is_empty() {
        return;
    }
    let Ok(shape) = shape_of(facade, &record.entity) else {
        return;
    };
    for (name, text) in &options.default_values {
        if shape.fields.contains_key(name.as_str()) && !record.fields.contains_key(name.as_str()) {
            record.fields.insert(shape.name(name), Value::string(text));
        }
    }
}

/// Java `defaultValues` on a data-file service call: each default the element does not set.
pub(super) fn apply_default_parameters(options: &LoadOptions, service: &mut DataFileService) {
    for (name, text) in &options.default_values {
        service.parameters.entry(name.clone()).or_insert_with(|| text.clone());
    }
}

type Shapes = crate::entity::ByDefinition<Arc<Shape>>;

/// Read once per entity name: every record of a file asks.
pub(super) fn shape_of(facade: &EntityFacade, entity_name: &str) -> Result<Arc<Shape>, Error> {
    let generation = crate::entity::definition_generation(facade);
    if let Some(shape) = facade.with_slot_read(Shapes::default, |shapes| shapes.get(entity_name, generation)) {
        return Ok(shape);
    }
    let shape = Arc::new(read_shape(facade, entity_name)?);
    facade.with_slot(Shapes::default, |shapes| shapes.insert(entity_name, generation, Arc::clone(&shape)));
    Ok(shape)
}

fn read_shape(facade: &EntityFacade, entity_name: &str) -> Result<Shape, Error> {
    facade.with_entity(entity_name, |entity, dialect, model| {
        // The names a value of the entity shares, so a record's fields and the value written share them too.
        let shared = (!entity.is_view).then(|| crate::entity::sql_cache::read_shape(facade, entity, dialect));
        let mut fields = HashMap::new();
        for field in &entity.fields {
            let name = shared
                .as_ref()
                .and_then(|shape| {
                    let at = shape.names.binary_search_by(|name| (**name).cmp(field.name.as_str())).ok()?;
                    Some(Arc::clone(&shape.names[at]))
                })
                .unwrap_or_else(|| Arc::from(field.name.as_str()));
            fields.insert(name, field.field_type.clone());
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
        let pk_names = entity
            .pk_field_names()
            .into_iter()
            .map(|name| {
                fields
                    .get_key_value(name)
                    .map_or_else(|| Arc::from(name), |(shared, _)| Arc::clone(shared))
            })
            .collect();
        let java_types = entity
            .fields
            .iter()
            .filter_map(|field| {
                let java = model.dictionary_type(&field.field_type)?.java_type.clone();
                Some((field.name.clone(), java))
            })
            .collect();
        Ok(Shape {
            full_name: entity.full_name.clone(),
            fields,
            pk_names,
            relations,
            java_types,
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

/// True when `components` is empty or names the component of `location`; a location that is not `component://` is
/// always in. Java's `componentNameList` narrows only the locations a load gathers itself, not one it is given.
pub fn location_in_components(location: &str, components: &[String]) -> bool {
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
