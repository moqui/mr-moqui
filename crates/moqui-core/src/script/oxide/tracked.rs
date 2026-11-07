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

//! Bare-name reads and writes of an oxide run over the context stack, and its tracked values: the oxide side of
//! `runtime.rs`'s `read_name`, `write_name` and `flush_tracked` (plans/Script-Engine-Oxide.md section 4).
//!
//! A list or map a run reads by name is converted once and tracked: the script changes the object, and the changes are
//! written back to the stack at a flush, when the object no longer matches what the stack holds. A flush is
//! [`Flush::Keep`] before a host call that reads the stack, [`Flush::Release`] before one that may change it (the
//! object is then released: tracked again at its next read by name, or at the end, while the stack still holds what was
//! written), and [`Flush::End`] at the end of the run. A decimal or an entity value read by name is kept for the run
//! too, so two reads are one object, but needs no write: a decimal does not change, and an entity value writes its
//! cell.

use std::cell::RefCell;
use std::collections::HashMap;

use moqui_lib::object::Value as HostValue;
use quickjs_oxide::engine::api::core::{Js, PropertyKey, Value};

use super::convert::{key, to_host, to_js, Conv};
use super::decimal::OxDecimal;
use super::engine::{current, OxideEngine};
use crate::script::run::STACK;
use crate::script::runtime::{converts_with_script, temporal_text, MAX_NESTING, SET_KEY, WRAP_KEY};

/// What a flush does with the tracked values after writing them back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Flush {
    /// Keep tracking them, for a host call that only reads the stack.
    Keep,
    /// Release them, for a host call that may change the stack.
    Release,
    /// Forget them, at the end of a run.
    End,
}

/// What a screen unit sets aside of the run around it ([`RunValues::take_outer`]): its released values and the values
/// it kept by name.
pub(super) type OuterValues = (HashMap<String, (Value, HostValue)>, HashMap<String, Value>);

/// The values of one run, by name.
#[derive(Default)]
pub(super) struct RunValues {
    /// Lists and maps read or assigned by name, written back at a flush.
    tracked: HashMap<String, Value>,
    /// Values a [`Flush::Release`] took off `tracked`, with the host value written for each.
    released: HashMap<String, (Value, HostValue)>,
    /// Decimals and entity values read by name, kept so a second read is the same object.
    kept: HashMap<String, Value>,
    /// Whether a flush is writing back now: a conversion meanwhile does not flush again.
    flushing: bool,
}

impl RunValues {
    /// Forget everything, at the end of a run.
    pub(super) fn clear(&mut self) {
        self.tracked.clear();
        self.released.clear();
        self.kept.clear();
        self.flushing = false;
    }

    /// Set aside what a screen unit must not see of the run around it (its released values, and the decimals and
    /// entity values kept by name, which the unit's context frame may name differently), for [`Self::restore_outer`].
    pub(super) fn take_outer(&mut self) -> OuterValues {
        (std::mem::take(&mut self.released), std::mem::take(&mut self.kept))
    }

    /// After a screen unit: what it tracked forgotten, and the run around it given back what [`Self::take_outer`] took.
    pub(super) fn restore_outer(&mut self, outer: OuterValues) {
        self.tracked.clear();
        (self.released, self.kept) = outer;
    }

    /// The lists and maps tracked now, for the debug check of an unflushed run.
    pub(super) fn tracked(&self) -> Vec<(String, Value)> {
        self.tracked
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }
}

fn values(engine: &OxideEngine) -> &RefCell<RunValues> {
    engine.run_values()
}

/// A bare `name` read: the run's own object for it, else the stack's value converted (a list or map tracked, a decimal
/// or entity value kept). `context` is the stack object; the helper objects QuickJS answers by name fall back.
pub(super) fn read_name<J: Js>(js: &mut J, name: &str) -> Result<Value, Conv> {
    if super::engine::fallback_pending() {
        return Err(Conv::Unsupported("a run that fell back"));
    }
    let engine = current().ok_or(Conv::Unsupported("no oxide engine"))?;
    {
        let values = values(&engine).borrow();
        if let Some(found) = values.tracked.get(name).or_else(|| values.kept.get(name)) {
            return Ok(found.clone());
        }
    }
    if let Some(readopted) = readopt(&engine, name) {
        return Ok(readopted);
    }
    // In place, without a copy of the stack's value, unless the conversion runs script code (a tools map's wrapper),
    // which could reach the stack: that value is copied out and converted with the stack free, as QuickJS's
    // `read_name` does (`converts_with_script`).
    let kind_of = |value: &HostValue| match value {
        HostValue::List(_) | HostValue::Map(_) => Some(true),
        HostValue::Decimal(_) | HostValue::Object(_) => Some(false),
        _ => None,
    };
    let held = STACK.with(|stack| {
        let stack = stack.borrow();
        stack.with_value(name, |value| {
            if converts_with_script(value) {
                Err(value.clone())
            } else {
                Ok(to_js(js, &engine, value).map(|converted| (converted, kind_of(value))))
            }
        })
    });
    let found = held.map(|held| match held {
        Ok(converted) => converted,
        Err(copied) => to_js(js, &engine, &copied).map(|converted| (converted, kind_of(&copied))),
    });
    match found {
        Some(converted) => {
            let (value, kind) = converted?;
            let mut values = values(&engine).borrow_mut();
            match kind {
                Some(true) => {
                    values.tracked.insert(name.to_string(), value.clone());
                }
                Some(false) => {
                    values.kept.insert(name.to_string(), value.clone());
                }
                None => {}
            }
            Ok(value)
        }
        None => super::names::missing(name),
    }
}

/// Forget the run's object of `name`, for `delete context.name`.
pub(super) fn forget(name: &str) -> Result<(), Conv> {
    if super::engine::fallback_pending() {
        return Err(Conv::Unsupported("a run that fell back"));
    }
    let engine = current().ok_or(Conv::Unsupported("no oxide engine"))?;
    let mut values = values(&engine).borrow_mut();
    values.tracked.remove(name);
    values.released.remove(name);
    values.kept.remove(name);
    Ok(())
}

/// A bare `name = value`: the value goes on the stack's top frame, and a plain array or object stays the script's
/// object, tracked, so `entry = list[i]; entry.field = x` changes the element as on QuickJS.
pub(super) fn write_name<J: Js>(js: &mut J, name: &str, value: &Value) -> Result<(), Conv> {
    if super::engine::fallback_pending() {
        return Err(Conv::Unsupported("a run that fell back"));
    }
    let engine = current().ok_or(Conv::Unsupported("no oxide engine"))?;
    {
        let mut values = values(&engine).borrow_mut();
        values.tracked.remove(name);
        values.released.remove(name);
        values.kept.remove(name);
    }
    let host = to_host(js, &engine, value)?;
    let keep = matches!(host, HostValue::List(_) | HostValue::Map(_)) && is_plain_collection(js, &engine, value)?;
    crate::script::census::note_side_effect();
    STACK.with(|stack| {
        stack
            .borrow_mut()
            .put(name, host)
            .map(|_| ())
            .map_err(|err| Conv::Error(err.message().to_string()))
    })?;
    if keep {
        values(&engine)
            .borrow_mut()
            .tracked
            .insert(name.to_string(), value.clone());
    }
    Ok(())
}

/// An array, or an object made by `{}` or `new Object()`: what converts to a host list or map and back as the same
/// shape.
fn is_plain_collection<J: Js>(js: &mut J, engine: &OxideEngine, value: &Value) -> Result<bool, Conv> {
    let Value::Object(object) = value else {
        return Ok(false);
    };
    if js.is_array_object(object)? {
        return Ok(true);
    }
    Ok(js.get_prototype_of(object)?.as_ref() == Some(&engine.protos.object_prototype))
}

/// Track a released value again when the stack still holds what was written for it.
fn readopt(engine: &OxideEngine, name: &str) -> Option<Value> {
    let mut values = values(engine).borrow_mut();
    let (value, written) = values.released.remove(name)?;
    if !STACK.with(|stack| stack.borrow().holds(name, &written)) {
        return None;
    }
    values.tracked.insert(name.to_string(), value.clone());
    Some(value)
}

/// Write the tracked lists and maps that changed back to the stack (see the module doc for the modes).
pub(super) fn flush<J: Js>(js: &mut J, mode: Flush) -> Result<(), Conv> {
    // a run that fell back writes nothing back: QuickJS runs it again from the start (the end of the run skips its
    // flush too); a host call's flush fails the call
    if super::engine::fallback_pending() {
        return Err(Conv::Unsupported("a run that fell back"));
    }
    let Some(engine) = current() else {
        return Ok(());
    };
    {
        let mut values = values(&engine).borrow_mut();
        if values.flushing {
            return Ok(());
        }
        values.flushing = true;
    }
    let result = flush_values(js, &engine, mode);
    values(&engine).borrow_mut().flushing = false;
    result
}

fn flush_values<J: Js>(js: &mut J, engine: &OxideEngine, mode: Flush) -> Result<(), Conv> {
    if mode == Flush::End {
        let names: Vec<String> = values(engine).borrow().released.keys().cloned().collect();
        for name in names {
            readopt(engine, &name);
        }
    }
    let entries: Vec<(String, Value)> = {
        let mut values = values(engine).borrow_mut();
        if mode == Flush::Keep {
            values.tracked()
        } else {
            values.tracked.drain().collect()
        }
    };
    let mut first = None;
    for (name, value) in entries {
        // What the script did not change is on the stack already: no conversion, no write. A release still notes
        // what the stack holds, for `readopt`.
        // The comparison reads the script's object, which may run script code (a Proxy's traps, a getter) that writes
        // the stack: it compares a copy, with the stack free.
        let held = STACK.with(|stack| stack.borrow().with_value(&name, HostValue::clone));
        let unchanged = held.map(|held| {
            matches_host(js, engine, &value, &held, 0)
                .map(|same| same.then(|| (mode == Flush::Release).then_some(held)))
        });
        match unchanged {
            Some(Ok(Some(written))) => {
                if let Some(written) = written {
                    values(engine).borrow_mut().released.insert(name, (value, written));
                }
                continue;
            }
            Some(Err(err)) => {
                first.get_or_insert(err);
                continue;
            }
            Some(Ok(None)) | None => {}
        }
        match to_host(js, engine, &value) {
            Ok(host) => {
                if mode == Flush::Release {
                    values(engine)
                        .borrow_mut()
                        .released
                        .insert(name.clone(), (value, host.clone()));
                }
                crate::script::census::note_side_effect();
                STACK.with(|stack| {
                    let _ = stack.borrow_mut().put_nearest(&name, host);
                });
            }
            Err(err) => {
                first.get_or_insert(err);
            }
        }
    }
    match first {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Whether `value`, a list or map a run that does not write read by name, still holds what the stack holds `held`, for
/// the debug check of an unflushed run (`engine::check_kept_names`). A tools map is compared by the stack's own keys
/// only, a member its wrapper made a method left out: the wrapper adds members (a Node's `childNodes` and
/// `attributes`, a Resource's `size` and `lastModified`, read by a host call), which QuickJS writes back as data at its
/// end and a later read here makes again, so they are no change of the script's; reading them would call the host.
pub(super) fn kept_matches<J: Js>(
    js: &mut J,
    engine: &OxideEngine,
    value: &Value,
    held: &HostValue,
    depth: usize,
) -> Result<bool, Conv> {
    if depth >= MAX_NESTING {
        return Ok(false);
    }
    // a map whose `__wrap` is not a name passes through as itself, its members unread (`convert::to_js`)
    let wrapped = |entries: &indexmap::IndexMap<String, HostValue>| {
        matches!(entries.get(WRAP_KEY), Some(HostValue::String(_))) && engine.tools_wrap().is_some()
    };
    let passes_through = matches!(held, HostValue::Map(entries) if entries.contains_key(WRAP_KEY) && !wrapped(entries));
    if !converts_with_script(held) || passes_through {
        return Ok(to_host(js, engine, value)? == *held);
    }
    let Value::Object(object) = value else {
        return Ok(false);
    };
    match held {
        HostValue::List(items) => {
            let length = js.get_property(object, &key("length")?)?;
            if !js.is_array_object(object)? || js.to_number(&length)? != items.len() as f64 {
                return Ok(false);
            }
            for (index, item) in items.iter().enumerate() {
                let Ok(index) = u32::try_from(index) else {
                    return Ok(false);
                };
                let element = js.get_property(object, &PropertyKey::index(index))?;
                if !kept_matches(js, engine, &element, item, depth + 1)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        HostValue::Map(entries) => {
            for (name, item) in entries {
                let found = js.get_property(object, &key(name)?)?;
                if let Value::Object(inner) = &found {
                    if js.as_callable(inner)?.is_some() {
                        continue;
                    }
                }
                if !kept_matches(js, engine, &found, item, depth + 1)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Whether `value` converts to `host`, read in place, as the QuickJS side's `js_matches_host`: `false` where it cannot
/// tell cheaply, and the value is converted.
fn matches_host<J: Js>(
    js: &mut J,
    engine: &OxideEngine,
    value: &Value,
    host: &HostValue,
    depth: usize,
) -> Result<bool, Conv> {
    if depth >= MAX_NESTING {
        return Ok(false);
    }
    Ok(match host {
        HostValue::Null => matches!(value, Value::Null | Value::Undefined),
        HostValue::Bool(flag) => matches!(value, Value::Bool(found) if found == flag),
        HostValue::String(text) => text_is(value, text),
        HostValue::I32(number) => matches!(value, Value::Int(found) if found == number),
        HostValue::I64(number) => match i32::try_from(*number) {
            Ok(small) => matches!(value, Value::Int(found) if *found == small),
            #[allow(clippy::cast_precision_loss)]
            Err(_) => matches!(value, Value::Float(found) if *found == *number as f64),
        },
        HostValue::F64(number) => matches!(value, Value::Float(found) if found == number),
        HostValue::Decimal(_) => match value {
            Value::Object(object) => js
                .host_data::<OxDecimal>(object)?
                .is_some_and(|decimal| decimal.inner == *host),
            _ => false,
        },
        HostValue::Timestamp(_) | HostValue::Date(_) | HostValue::Time(_) => text_is(value, &temporal_text(host)),
        HostValue::List(items) => {
            let Value::Object(object) = value else {
                return Ok(false);
            };
            if !js.is_array_object(object)? {
                return Ok(false);
            }
            let length = js.get_property(object, &key("length")?)?;
            if js.to_number(&length)? != items.len() as f64 {
                return Ok(false);
            }
            for (index, item) in items.iter().enumerate() {
                let Ok(index) = u32::try_from(index) else {
                    return Ok(false);
                };
                let element = js.get_property(object, &PropertyKey::index(index))?;
                if !matches_host(js, engine, &element, item, depth + 1)? {
                    return Ok(false);
                }
            }
            true
        }
        HostValue::Map(entries) => {
            if entries.contains_key(SET_KEY) || entries.contains_key(WRAP_KEY) {
                return Ok(false);
            }
            let Value::Object(object) = value else {
                return Ok(false);
            };
            if js.get_prototype_of(object)?.as_ref() != Some(&engine.protos.object_prototype) {
                return Ok(false);
            }
            // the properties in order, as the conversion takes them, functions left out as it leaves them out
            let mut expected = entries.iter();
            for property in js.own_property_keys(object)? {
                let Some(name) = property.to_js_string() else {
                    continue;
                };
                let Some(descriptor) = js.get_own_property(object, &property)? else {
                    continue;
                };
                if !descriptor.enumerable() {
                    continue;
                }
                // a getter runs script code, which may write the stack this check holds: converted as before
                let quickjs_oxide::engine::api::core::CompleteOrdinaryPropertyDescriptor::Data { value: item, .. } =
                    descriptor
                else {
                    return Ok(false);
                };
                if let Value::Object(inner) = &item {
                    if js.as_callable(inner)?.is_some() {
                        continue;
                    }
                }
                let Some((held_name, held_item)) = expected.next() else {
                    return Ok(false);
                };
                if name.to_utf8_lossy() != *held_name || !matches_host(js, engine, &item, held_item, depth + 1)? {
                    return Ok(false);
                }
            }
            expected.next().is_none()
        }
        HostValue::Bytes(_) | HostValue::Object(_) => false,
    })
}

fn text_is(value: &Value, text: &str) -> bool {
    match value {
        Value::String(found) => found.to_utf8_lossy() == text,
        _ => false,
    }
}
