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

//! `EntityDataLoader.check()`: read entity data as a load would and compare each record, nested records included,
//! with the database instead of writing it. Java's `CheckValueHandler` and `EntityValueBase.checkAgainstDatabase`.
//!
//! The readers hand every record to `store_record` and every service element to `call_data_file_service`. While a
//! check runs on this thread those divert here, so XML, CSV, JSON and zip data all check the same way.
//!
//! `EntityDataLoader.list()` (Java's `ListValueHandler`) diverts the same way: the records become entity values, in
//! the order a load would write them, and nothing is checked, written or called.

use std::cell::RefCell;
use std::collections::BTreeMap;

use indexmap::IndexMap;
use moqui_lib::error::Error;
use moqui_lib::object::{basic_convert, compare_obj, to_plain_string, Value};

use super::loader::{convert_fields, inherit_pk_fields, load_entity_data, load_entity_xml_text, Record};
use super::{DataFileService, LoadOptions};
use crate::entity::FieldMap;
use crate::entity::facade::EntityFacade;
use crate::entity::EntityValue;

/// What a check found: one message for each record not found and each field that did not match (with
/// [`check_info_with`] a row in `diff_info` instead), and how many fields were compared.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CheckResult {
    pub messages: Vec<String>,
    pub fields_checked: u64,
    /// Java's `checkInfo` rows: `entity`, `pk`, `notFound`, `pkComplete` and `location`, with `createValues` for a
    /// record not found, or `field`, `value` and `dbValue` for a field that did not match.
    pub diff_info: Vec<IndexMap<String, Value>>,
    /// With [`list_with`], the values of the records read, nested records after the record they are in.
    pub values: Vec<EntityValue>,
}

/// The check running on this thread.
struct CheckState {
    result: CheckResult,
    /// Java's `checkInfo`: rows in `diff_info`, not messages.
    info: bool,
    /// Java's `list`: the records become values in `values` and are not checked.
    list: bool,
    /// The file or text being read, for the rows.
    location: String,
}

thread_local! {
    static CHECKING: RefCell<Option<CheckState>> = const { RefCell::new(None) };
}

/// Ends the check of this thread, also on an error or a panic.
struct CheckScope;

impl CheckScope {
    fn enter(info: bool, list: bool) -> Result<Self, Error> {
        CHECKING.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_some() {
                return Err(Error::new("a data check is already running on this thread"));
            }
            *slot = Some(CheckState {
                result: CheckResult::default(),
                info,
                list,
                location: String::new(),
            });
            Ok(CheckScope)
        })
    }

    fn finish(self) -> CheckResult {
        CHECKING
            .with(|slot| slot.borrow_mut().take())
            .map(|state| state.result)
            .unwrap_or_default()
    }
}

impl Drop for CheckScope {
    fn drop(&mut self) {
        CHECKING.with(|slot| slot.borrow_mut().take());
    }
}

/// Run `body`, a load of any kind, as a check: the records it reads are compared with the database and none is
/// written.
pub fn check_with<R>(body: impl FnOnce() -> Result<R, Error>) -> Result<(R, CheckResult), Error> {
    let scope = CheckScope::enter(false, false)?;
    let out = body()?;
    Ok((out, scope.finish()))
}

/// Run `body` as Java's `checkInfo`: like [`check_with`], but what did not match is a row of
/// [`CheckResult::diff_info`] rather than a message.
pub fn check_info_with<R>(body: impl FnOnce() -> Result<R, Error>) -> Result<(R, CheckResult), Error> {
    let scope = CheckScope::enter(true, false)?;
    let out = body()?;
    Ok((out, scope.finish()))
}

/// Run `body`, a load of any kind, as Java's `EntityDataLoader.list()`: the records it reads come back as values in
/// [`CheckResult::values`], and none is checked or written. A service element is logged and not called.
pub fn list_with<R>(body: impl FnOnce() -> Result<R, Error>) -> Result<(R, CheckResult), Error> {
    let scope = CheckScope::enter(false, true)?;
    let out = body()?;
    Ok((out, scope.finish()))
}

/// Check entity XML text against the database, as `makeDataLoader().xmlText(text).check()`.
pub fn check_entity_xml_text(
    facade: &EntityFacade,
    location: &str,
    text: &str,
    options: &LoadOptions,
) -> Result<CheckResult, Error> {
    check_with(|| load_entity_xml_text(facade, location, text, options)).map(|(_, result)| result)
}

/// Check the data of one location, a file or a directory, as `makeDataLoader().location(location).check()`. Each
/// file adds a last message that names it, as Java's.
pub fn check_entity_data(facade: &EntityFacade, location: &str, options: &LoadOptions) -> Result<CheckResult, Error> {
    check_with(|| load_entity_data(facade, location, options)).map(|(_, result)| result)
}

/// True while a check runs on this thread: the readers then check records instead of storing them.
pub(super) fn checking() -> bool {
    CHECKING.with(|slot| slot.borrow().is_some())
}

/// Run `body`, the read of `location` (a file or text), with the check's rows naming it.
pub(super) fn at_location<R>(location: &str, body: impl FnOnce() -> R) -> R {
    let before = CHECKING.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .map(|state| std::mem::replace(&mut state.location, location.to_string()))
    });
    let out = body();
    if let Some(before) = before {
        CHECKING.with(|slot| {
            if let Some(state) = slot.borrow_mut().as_mut() {
                state.location = before;
            }
        });
    }
    out
}

/// Run `body`, the check of the file `location`. As Java's `loadSingleFile`, a `-- Checked data` message follows it
/// unless the file gave no messages and the load hides those (`messageNoActionFiles(false)`).
pub(super) fn check_file(
    location: &str,
    options: &LoadOptions,
    body: impl FnOnce() -> Result<u64, Error>,
) -> Result<u64, Error> {
    let before = CHECKING.with(|slot| slot.borrow().as_ref().map(|state| state.result.messages.len()));
    let Some(before) = before else {
        return body();
    };
    let records = at_location(location, body)?;
    CHECKING.with(|slot| {
        if let Some(state) = slot.borrow_mut().as_mut() {
            if !options.hide_no_action_files || state.result.messages.len() > before {
                state
                    .result
                    .messages
                    .push(format!("-- Checked data ({records} records) in {location}"));
            }
        }
    });
    Ok(records)
}

/// Check one record read from the data and its nested records. Returns how many records it read, as a store
/// returns how many it wrote.
pub(super) fn check_record(facade: &EntityFacade, record: &Record) -> Result<u64, Error> {
    let (info, list, location) = CHECKING.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|state| (state.info, state.list, state.location.clone()))
            .unwrap_or_default()
    });
    if list {
        let mut values = Vec::new();
        let records = list_tree(facade, record, &BTreeMap::new(), &mut values)?;
        CHECKING.with(|slot| {
            if let Some(state) = slot.borrow_mut().as_mut() {
                state.result.values.extend(values);
            }
        });
        return Ok(records);
    }
    let mut found = Findings {
        info,
        location,
        ..Findings::default()
    };
    let records = check_tree(facade, record, &BTreeMap::new(), &mut found)?;
    CHECKING.with(|slot| {
        if let Some(state) = slot.borrow_mut().as_mut() {
            state.result.messages.extend(found.messages);
            state.result.diff_info.extend(found.diff_info);
            state.result.fields_checked = state.result.fields_checked.saturating_add(found.fields_checked);
        }
    });
    Ok(records)
}

/// A service element is not called by a check or a list, as Java's.
pub(super) fn skip_service(service: &DataFileService) {
    if CHECKING.with(|slot| slot.borrow().as_ref().is_some_and(|state| state.list)) {
        tracing::warn!(
            "For load to EntityList not calling service [{}] with parameters {:?}",
            service.name,
            service.parameters
        );
        return;
    }
    let message = format!(
        "Doing check only so not calling service [{}] with parameters {:?}",
        service.name, service.parameters
    );
    CHECKING.with(|slot| {
        if let Some(state) = slot.borrow_mut().as_mut() {
            state.result.messages.push(message);
        }
    });
}

/// The values of one record tree, the record first and then its nested records, as Java's
/// `addValuesFromPlainMapRecursive`: each nested record gets the keys of the records it is in, as a load passes them.
fn list_tree(
    facade: &EntityFacade,
    record: &Record,
    ancestor: &FieldMap,
    values: &mut Vec<EntityValue>,
) -> Result<u64, Error> {
    let mut fields = record.fields.clone();
    inherit_pk_fields(facade, &record.entity, ancestor, &mut fields)?;
    convert_fields(facade, &record.entity, &mut fields)?;
    let pk_names = facade.with_entity(&record.entity, |entity, _, _| {
        Ok(entity
            .pk_field_names()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>())
    })?;
    let pk: Vec<(String, Value)> = pk_names
        .iter()
        .map(|name| (name.clone(), fields.get(name.as_str()).cloned().unwrap_or(Value::Null)))
        .collect();
    let mut value = facade.make_value(&record.entity)?;
    for (name, field) in fields {
        value.set(&name, field);
    }
    values.push(value);
    let shared = shared_keys(pk, ancestor);
    let mut records = 1_u64;
    for child in &record.children {
        records = records.saturating_add(list_tree(facade, child, &shared, values)?);
    }
    Ok(records)
}

/// The keys a record passes to its nested records: its own primary key, then those of the records it is in.
fn shared_keys(pk: Vec<(String, Value)>, ancestor: &FieldMap) -> FieldMap {
    let mut shared: FieldMap = pk
        .into_iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(name, value)| (std::sync::Arc::from(name), value))
        .collect();
    for (name, value) in ancestor {
        if !value.is_null() {
            shared.entry(name.clone()).or_insert_with(|| value.clone());
        }
    }
    shared
}

/// What the check of one record tree found.
#[derive(Default)]
struct Findings {
    info: bool,
    location: String,
    messages: Vec<String>,
    diff_info: Vec<IndexMap<String, Value>>,
    fields_checked: u64,
}

impl Findings {
    /// A `checkInfo` row with the fields every row has.
    fn row(&self, entity: &str, pk: &[(String, Value)], not_found: bool) -> IndexMap<String, Value> {
        let mut row = IndexMap::new();
        row.insert("entity".to_string(), Value::string(entity));
        row.insert(
            "pk".to_string(),
            Value::Map(pk.iter().map(|(name, value)| (name.clone(), value.clone())).collect()),
        );
        row.insert("notFound".to_string(), Value::Bool(not_found));
        row.insert(
            "pkComplete".to_string(),
            Value::Bool(pk.iter().all(|(_, value)| !value.is_null())),
        );
        row.insert("location".to_string(), Value::string(&self.location));
        row
    }
}

fn check_tree(
    facade: &EntityFacade,
    record: &Record,
    ancestor: &FieldMap,
    found: &mut Findings,
) -> Result<u64, Error> {
    let mut fields = record.fields.clone();
    inherit_pk_fields(facade, &record.entity, ancestor, &mut fields)?;
    convert_fields(facade, &record.entity, &mut fields)?;
    let (pk_names, non_pk_names) = facade.with_entity(&record.entity, |entity, _, _| {
        let pk_names: Vec<String> = entity.pk_field_names().into_iter().map(str::to_string).collect();
        let non_pk_names: Vec<String> = entity
            .fields
            .iter()
            .filter(|field| !field.is_pk && field.name != "lastUpdatedStamp")
            .map(|field| field.name.clone())
            .collect();
        Ok((pk_names, non_pk_names))
    })?;
    let pk: Vec<(String, Value)> = pk_names
        .iter()
        .map(|name| (name.clone(), fields.get(name.as_str()).cloned().unwrap_or(Value::Null)))
        .collect();
    let pk_text = pk_display(&pk);
    let db = if pk.iter().any(|(_, value)| value.is_null()) {
        None
    } else {
        let mut find = facade.find(&record.entity)?;
        for (name, value) in &pk {
            find = find.eq(name, value.clone());
        }
        find.one()?
    };
    match db {
        None if found.info => {
            let mut row = found.row(&record.entity, &pk, true);
            row.insert(
                "createValues".to_string(),
                Value::Map(
                    fields
                        .iter()
                        .map(|(name, value)| (name.to_string(), value.clone()))
                        .collect(),
                ),
            );
            found.diff_info.push(row);
        }
        None => found.messages.push(format!(
            "Entity {} record not found for primary key {pk_text}",
            record.entity
        )),
        Some(db) => {
            for name in &non_pk_names {
                found.fields_checked = found.fields_checked.saturating_add(1);
                let Some(check) = fields.get(name.as_str()).filter(|value| !value.is_null()) else {
                    continue;
                };
                let stored = db.get(name).cloned().unwrap_or(Value::Null);
                if same(check, &stored) {
                    continue;
                }
                if found.info {
                    let mut row = found.row(&record.entity, &pk, false);
                    row.insert("field".to_string(), Value::string(name));
                    row.insert("value".to_string(), check.clone());
                    row.insert("dbValue".to_string(), stored);
                    found.diff_info.push(row);
                } else {
                    found.messages.push(format!(
                        "Field {}.{name} did not match; check (file) value [{}], db value [{}] for primary key {pk_text}",
                        record.entity,
                        value_display(check),
                        value_display(&stored)
                    ));
                }
            }
        }
    }
    // The nested records get this record's keys, as a load passes them down.
    let shared = shared_keys(pk, ancestor);
    let mut records = 1_u64;
    for child in &record.children {
        records = records.saturating_add(check_tree(facade, child, &shared, found)?);
    }
    Ok(records)
}

/// Java compares with `compareTo` when it can, so `1.50` matches `1.5`; otherwise with `equals`. A field the database
/// holds as null never matches a value.
fn same(check: &Value, stored: &Value) -> bool {
    if stored.is_null() {
        return false;
    }
    match compare_obj(check, stored) {
        Ok(order) => order == std::cmp::Ordering::Equal,
        // Text the field type did not take, such as `1800.0` for an integer field, still matches by number.
        Err(_) => match (as_decimal(check), as_decimal(stored)) {
            (Some(left), Some(right)) => compare_obj(&left, &right).is_ok_and(|order| order.is_eq()),
            _ => to_plain_string(check) == to_plain_string(stored),
        },
    }
}

fn as_decimal(value: &Value) -> Option<Value> {
    let converted = basic_convert(value, "BigDecimal");
    matches!(converted.value, Value::Decimal(_)).then_some(converted.value)
}

/// Java's `Map.toString` of the primary key: `{orderId=55500, orderItemSeqId=01}`.
fn pk_display(pk: &[(String, Value)]) -> String {
    let parts: Vec<String> = pk
        .iter()
        .map(|(name, value)| format!("{name}={}", value_display(value)))
        .collect();
    format!("{{{}}}", parts.join(", "))
}

/// A time stamp shows as its date and time, which reads better in a message than the epoch millis a plain string
/// gives.
fn value_display(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Timestamp(stamp) => stamp.to_string(),
        other => to_plain_string(other),
    }
}
