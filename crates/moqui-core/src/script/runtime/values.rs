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

//! Reading context-stack values from Rust: entity values and lists as maps and items, and frame
//! calls the screen renderer makes between its own steps.

use moqui_lib::sync::lock;

use indexmap::IndexMap;

use super::*;

/// The fields of a map or an entity value, an entity value's localized fields in the user's locale and a view's
/// `pq-expression` aliases, as Java's `getMap`.
pub(crate) fn value_map(value: &Value) -> Option<IndexMap<String, Value>> {
    match value {
        Value::Map(entries) => Some(entries.clone()),
        Value::Object(object) => {
            let cell = object.downcast::<EntityCell>()?;
            let guard = lock(&cell);
            if !guard.has_l10n() {
                return Some(
                    guard
                        .fields()
                        .iter()
                        .map(|(key, item)| (key.to_string(), item.clone()))
                        .collect(),
                );
            }
            // The lookups are finds and an expression may read this value, so they run with the value unlocked.
            let copy = guard.clone();
            drop(guard);
            let fields = match entity_facade() {
                Some(facade) => copy.localized_fields(&facade),
                None => copy.fields().clone(),
            };
            Some(fields.into_iter().map(|(key, item)| (key.to_string(), item)).collect())
        }
        _ => None,
    }
}

/// The stored fields of a map or an entity value, not localized: what a comparison of values reads.
pub(crate) fn value_map_stored(value: &Value) -> Option<IndexMap<String, Value>> {
    match value {
        Value::Map(entries) => Some(entries.clone()),
        Value::Object(object) => {
            let cell = object.downcast::<EntityCell>()?;
            let guard = lock(&cell);
            Some(
                guard
                    .fields()
                    .iter()
                    .map(|(key, item)| (key.to_string(), item.clone()))
                    .collect(),
            )
        }
        _ => None,
    }
}

/// One field of a map or entity value, an entity value's localized field in the user's locale or `pq-expression`
/// alias. None when the field is not there.
pub(crate) fn value_field(value: &Value, field: &str) -> Option<Value> {
    match value {
        Value::Map(entries) => entries.get(field).cloned(),
        Value::Object(object) => {
            let cell = object.downcast::<EntityCell>()?;
            let guard = lock(&cell);
            if !guard.localizes(field) {
                return guard.get(field).cloned();
            }
            let Some(facade) = entity_facade() else {
                return guard.get(field).cloned();
            };
            let read = guard.read_localized(&facade, field);
            drop(guard);
            match read {
                Ok(found) => found,
                Err(read) => read.run(&facade),
            }
        }
        _ => None,
    }
}

fn entity_facade() -> Option<crate::entity::EntityFacade> {
    ExecutionContext::with_current(|ec| ec.factory().entity()).flatten()
}

/// The entity name of an entity value.
pub(crate) fn value_entity_name(value: &Value) -> Option<String> {
    let Value::Object(object) = value else {
        return None;
    };
    let cell = object.downcast::<EntityCell>()?;
    let guard = lock(&cell);
    Some(guard.entity_name().to_string())
}

/// The rows of an entity list a script made, as they are now. `None` for any other value.
pub fn script_list_values(value: &Value) -> Option<Vec<EntityValue>> {
    let Value::Object(object) = value else {
        return None;
    };
    object.downcast::<ScriptList>().map(|list| list.values())
}

/// The items of a list, an entity list, or the values of a map.
pub(crate) fn value_items(value: &Value) -> Option<Vec<Value>> {
    match value {
        Value::List(items) => Some(items.clone()),
        Value::Map(entries) => Some(entries.values().cloned().collect()),
        Value::Object(object) => {
            // A script's list hands out its own rows, so an `<iterate>` entry is the row of the list.
            if let Some(list) = object.downcast::<ScriptList>() {
                return Some(
                    list.cells()
                        .iter()
                        .map(|cell| Value::Object(HostObject(Arc::clone(cell) as Arc<dyn std::any::Any + Send + Sync>)))
                        .collect(),
                );
            }
            let list = object.downcast::<EntityList>()?;
            Some(list.values().iter().map(|item| entity_value(item.clone())).collect())
        }
        _ => None,
    }
}

/// An entity value as a context-stack value, shared by reference with scripts.
fn entity_value(value: EntityValue) -> Value {
    Value::Object(HostObject(Arc::new(Mutex::new(value))))
}

/// Pop the top frame and return what it held.
pub(crate) fn pop_context_frame() -> IndexMap<String, Value> {
    STACK.with(|stack| {
        stack
            .borrow_mut()
            .pop()
            .ok()
            .flatten()
            .map(|frame| frame.borrow().clone())
            .unwrap_or_default()
    })
}

/// Put every entry in the top frame.
pub(crate) fn put_context_all(entries: &IndexMap<String, Value>) {
    STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        for (key, value) in entries {
            let _ = stack.put(key.clone(), value.clone());
        }
    });
}

/// The combined visible names.
pub(crate) fn context_combined() -> IndexMap<String, Value> {
    STACK.with(|stack| stack.borrow().combined_map())
}
