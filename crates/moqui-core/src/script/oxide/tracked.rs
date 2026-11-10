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
//! `runtime.rs`'s `read_name`, `write_name` and `flush_tracked` (plans/archive/Script-Engine-Oxide.md section 4).
//!
//! A list or map a run reads by name is converted once and tracked: the script changes the object, and the changes are
//! written back to the stack at a flush, when the object no longer matches what the stack holds. A flush is
//! [`Flush::Keep`] before a host call that reads the stack, [`Flush::Release`] before one that may change it (the
//! object is then released: tracked again at its next read by name, or at the end, while the stack still holds what was
//! written), and [`Flush::End`] at the end of the run. A decimal or an entity value read by name is kept too, until the
//! next flush that is not [`Flush::Keep`], so two reads between host calls are one object, but needs no write: a
//! decimal does not change, and an entity value writes its cell.

use std::cell::RefCell;
use std::collections::HashMap;

use moqui_lib::object::Value as HostValue;
use quickjs_oxide::engine::api::core::{
    CompleteOrdinaryPropertyDescriptor, DataRef, Js, OrdinaryPropertyDescriptor, PropertyKey, Value,
};

use super::convert::{key, to_host, to_js, Conv};
use super::decimal::OxDecimal;
use super::engine::{current, OxideEngine};
use crate::script::run::STACK;
use crate::script::runtime::{converts_with_script, has_special_key, temporal_text, MAX_NESTING, SET_KEY, WRAP_KEY};

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
pub(super) type OuterValues = (
    HashMap<String, (Value, u64), foldhash::fast::RandomState>,
    HashMap<String, Value, foldhash::fast::RandomState>,
);

/// Whether the stack still holds, for `name`, the host value whose [`fingerprint`] a [`Flush::Release`] noted.
fn print_held(name: &str, print: u64) -> bool {
    STACK.with(|stack| stack.borrow().with_value(name, fingerprint) == Some(print))
}

/// The values of one run, by name.
#[derive(Default)]
pub(super) struct RunValues {
    /// Lists, maps and decimals read or assigned by name, written back at a flush.
    tracked: HashMap<String, Value, foldhash::fast::RandomState>,
    /// Values a [`Flush::Release`] took off `tracked`, with the [`fingerprint`] of the host value written for each.
    released: HashMap<String, (Value, u64), foldhash::fast::RandomState>,
    /// Entity values read by name, kept so a second read is the same object.
    kept: HashMap<String, Value, foldhash::fast::RandomState>,
    /// Whether a flush is writing back now: a conversion meanwhile does not flush again.
    flushing: bool,
    /// Bare names read or written through the hooks since the bindings were last emptied, and how often: a name's
    /// second access binds it ([`note_access`]).
    accesses: HashMap<String, u8, foldhash::fast::RandomState>,
    /// The names bound as own data properties of the global object, which the engine's global sites read and write
    /// from their cache without the hooks (plans/Profiling3.md S2).
    bindings: Vec<Binding>,
    /// Whether this run binds names: one that writes them back at its end ([`super::engine::with_run`]).
    pub(super) binds: bool,
}

/// A bare name kept as an own data property of the global object, a write-back cache of its stack value.
pub(super) struct Binding {
    name: String,
    key: PropertyKey,
    /// The value last written to or read from the stack: a flush writes the binding back when it differs.
    synced: Value,
    /// The binding's value while a nested run has it off the global object ([`set_aside_bindings`]).
    parked: Option<Value>,
}

impl RunValues {
    /// Forget everything, at the end of a run.
    pub(super) fn clear(&mut self) {
        self.tracked.clear();
        self.released.clear();
        self.kept.clear();
        self.flushing = false;
        self.accesses.clear();
        self.bindings.clear();
        self.binds = false;
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

/// A bare `name` read: the run's own object for it, else the stack's value converted (a list, map or decimal tracked,
/// an entity value kept). `context` is the stack object; the helper objects QuickJS answers by name fall back.
pub(super) fn read_name<J: Js>(js: &mut J, name: &str) -> Result<Value, Conv> {
    if super::engine::fallback_pending() {
        return Err(Conv::Unsupported("a run that fell back"));
    }
    let engine = current().ok_or(Conv::Unsupported("no oxide engine"))?;
    // a bound name read through `context`: the binding is the current value
    let bound = values(&engine)
        .borrow()
        .bindings
        .iter()
        .find(|binding| binding.name == name)
        .map(|binding| binding.key.clone());
    if let Some(key) = bound {
        let global = js.global_object()?;
        if let Some(CompleteOrdinaryPropertyDescriptor::Data { value, .. }) = js.get_own_property(&global, &key)? {
            return Ok(value);
        }
    }
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
        // a decimal is tracked as a list is, so a releasing flush notes it and a read after the host call is the same
        // object while the stack holds an equal decimal, as QuickJS's was
        HostValue::List(_) | HostValue::Map(_) | HostValue::Decimal(_) => Some(true),
        HostValue::Object(_) => Some(false),
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
pub(super) fn forget<J: Js>(js: &mut J, name: &str) -> Result<(), Conv> {
    if super::engine::fallback_pending() {
        return Err(Conv::Unsupported("a run that fell back"));
    }
    let engine = current().ok_or(Conv::Unsupported("no oxide engine"))?;
    let mut values = values(&engine).borrow_mut();
    values.tracked.remove(name);
    values.released.remove(name);
    values.kept.remove(name);
    // a bound name goes off the global object too, so a bare read of it asks the stack again
    let unbound: Vec<PropertyKey> = values
        .bindings
        .iter()
        .filter(|binding| binding.name == name && binding.parked.is_none())
        .map(|binding| binding.key.clone())
        .collect();
    values.bindings.retain(|binding| binding.name != name);
    values.accesses.remove(name);
    drop(values);
    if !unbound.is_empty() {
        let global = js.global_object()?;
        for key in &unbound {
            js.delete_property(&global, key)?;
        }
    }
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
    // a bound name written through `context`, or written back by a flush: the binding has the value, written
    let bound = {
        let mut values = values(&engine).borrow_mut();
        values.bindings.iter_mut().find(|binding| binding.name == name).map(|binding| {
            binding.synced = value.clone();
            binding.key.clone()
        })
    };
    if let Some(key) = bound {
        let global = js.global_object()?;
        js.set_property(&global, &key, value.clone())?;
    }
    Ok(())
}

/// A bare name the hooks read or wrote, `value` its value now: at its [`BIND_AT`] access since the bindings were
/// emptied it is bound, an own data property of the global object holding `value`, so the engine's cached global sites
/// read and write it without the hooks until a flush that may change the stack. A body that reads a name a few times
/// pays a count.
pub(super) fn note_access<J: Js>(js: &mut J, name: &str, value: &Value) -> Result<(), Conv> {
    let Some(engine) = current() else {
        return Ok(());
    };
    {
        let mut values = values(&engine).borrow_mut();
        if !values.binds || values.bindings.len() >= MAX_BINDINGS || values.flushing {
            return Ok(());
        }
        let count = values.accesses.entry(name.to_string()).or_insert(0);
        *count = count.saturating_add(1);
        if *count != BIND_AT {
            return Ok(());
        }
    }
    let key = key(name)?;
    let global = js.global_object()?;
    let descriptor = OrdinaryPropertyDescriptor::data(value.clone(), true, false, true);
    if !js.define_own_property(&global, &key, &descriptor)? {
        return Ok(());
    }
    values(&engine).borrow_mut().bindings.push(Binding {
        name: name.to_string(),
        key,
        synced: value.clone(),
        parked: None,
    });
    Ok(())
}

/// The access of a name, since the bindings were last emptied, that binds it. A binding costs about what a few reads
/// through the hooks do (it is defined, later deleted, and the global sites fill again), so a name read fewer times
/// between two flushes that empty the bindings (a body around a service call in a loop) stays with the hooks.
const BIND_AT: u8 = 4;

/// The bindings a run keeps at most; past them a name goes through the hooks as before.
const MAX_BINDINGS: usize = 256;

/// Write back each binding whose value is not the one last synced, then, unless `keep`, take every binding off the
/// global object, so the next read of the name asks the stack again.
fn flush_bindings<J: Js>(js: &mut J, engine: &OxideEngine, keep: bool) -> Result<(), Conv> {
    let bound: Vec<(String, PropertyKey, Value)> = {
        let values = values(engine).borrow();
        if values.bindings.is_empty() {
            if !keep {
                drop(values);
                values_mut_clear_accesses(engine);
            }
            return Ok(());
        }
        values
            .bindings
            .iter()
            .filter(|binding| binding.parked.is_none())
            .map(|binding| (binding.name.clone(), binding.key.clone(), binding.synced.clone()))
            .collect()
    };
    let global = js.global_object()?;
    for (name, key, synced) in &bound {
        let Some(CompleteOrdinaryPropertyDescriptor::Data { value, .. }) = js.get_own_property(&global, key)? else {
            continue;
        };
        if !value.strict_equal(synced) {
            write_name(js, name, &value)?;
        }
    }
    if !keep {
        let taken = std::mem::take(&mut values(engine).borrow_mut().bindings);
        for binding in taken.iter().filter(|binding| binding.parked.is_none()) {
            js.delete_property(&global, &binding.key)?;
        }
        values_mut_clear_accesses(engine);
    }
    Ok(())
}

fn values_mut_clear_accesses(engine: &OxideEngine) {
    values(engine).borrow_mut().accesses.clear();
}

/// Before a nested run: the outer run's bindings come off the global object, their values kept, so the nested run's
/// names are its own context's; [`restore_bindings`] puts them back when it returns. Nothing is written to the stack
/// here, as the outer run's tracked values are not (the host call may have pushed another frame).
pub(super) fn set_aside_bindings<J: Js>(js: &mut J, engine: &OxideEngine) -> Result<(), Conv> {
    let keys: Vec<PropertyKey> = values(engine)
        .borrow()
        .bindings
        .iter()
        .filter(|binding| binding.parked.is_none())
        .map(|binding| binding.key.clone())
        .collect();
    if keys.is_empty() {
        return Ok(());
    }
    let global = js.global_object()?;
    let mut current = Vec::with_capacity(keys.len());
    for key in &keys {
        let value = match js.get_own_property(&global, key)? {
            Some(CompleteOrdinaryPropertyDescriptor::Data { value, .. }) => value,
            _ => Value::Undefined,
        };
        js.delete_property(&global, key)?;
        current.push(value);
    }
    let mut values = values(engine).borrow_mut();
    let mut current = current.into_iter();
    for binding in values.bindings.iter_mut().filter(|binding| binding.parked.is_none()) {
        binding.parked = current.next();
    }
    Ok(())
}

/// After a nested run: the outer run's bindings back on the global object with the values they had.
pub(super) fn restore_bindings<J: Js>(js: &mut J, engine: &OxideEngine) -> Result<(), Conv> {
    let parked: Vec<(PropertyKey, Value)> = values(engine)
        .borrow_mut()
        .bindings
        .iter_mut()
        .filter_map(|binding| binding.parked.take().map(|value| (binding.key.clone(), value)))
        .collect();
    if parked.is_empty() {
        return Ok(());
    }
    let global = js.global_object()?;
    for (key, value) in parked {
        let descriptor = OrdinaryPropertyDescriptor::data(value, true, false, true);
        js.define_own_property(&global, &key, &descriptor)?;
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
    if !print_held(name, written) {
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
    // the bound names first: one changed is written as any bare write is, which the tracked values below then see
    flush_bindings(js, engine, mode == Flush::Keep)?;
    // a host call that may change the stack may put another value under a kept name (a form-list's next row, a row
    // action's find): the next read takes the stack's value again, as QuickJS reads an entity value from the stack
    // each time (a decimal is tracked, and released below)
    if mode != Flush::Keep {
        values(engine).borrow_mut().kept.clear();
    }
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
        // A value of ordinary objects and data properties only is compared in place, under the stack's borrow: reading
        // it runs no code. A keep or an end then needs no copy of the stack's value.
        // A release then notes the stack's value by its fingerprint, with no copy.
        let release = mode == Flush::Release;
        let in_place = STACK.with(|stack| {
            stack.borrow().with_value(&name, |held| {
                matches_in_place(js, engine, &value, held, 0)
                    .map(|same| same.map(|same| (same, (same && release).then(|| fingerprint(held)))))
            })
        });
        match in_place {
            Some(Ok(Some((true, print)))) => {
                if let Some(print) = print {
                    values(engine).borrow_mut().released.insert(name, (value, print));
                }
                continue;
            }
            Some(Err(err)) => {
                first.get_or_insert(err);
                continue;
            }
            Some(Ok(Some((false, _)) | None)) | None => {}
        }
        // Otherwise the comparison reads the script's object, which may run script code (a Proxy's traps, a getter)
        // that writes the stack: it compares a copy, with the stack free.
        let held = STACK.with(|stack| stack.borrow().with_value(&name, HostValue::clone));
        let unchanged = held.map(|held| {
            matches_host(js, engine, &value, &held, 0)
                .map(|same| same.then(|| (mode == Flush::Release).then_some(held)))
        });
        match unchanged {
            Some(Ok(Some(written))) => {
                if let Some(written) = written {
                    values(engine).borrow_mut().released.insert(name, (value, fingerprint(&written)));
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
                    let print = fingerprint(&host);
                    values(engine).borrow_mut().released.insert(name.clone(), (value, print));
                }
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
    let wrapped = |entries: &moqui_lib::object::ValueMap| {
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
            let length = js.get_property(object, &super::convert::static_key("length")?)?;
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
    if let Some(same) = scalar_matches(value, host) {
        return Ok(same);
    }
    // an object from here
    Ok(match host {
        HostValue::Null
        | HostValue::Bool(_)
        | HostValue::String(_)
        | HostValue::I32(_)
        | HostValue::I64(_)
        | HostValue::F64(_)
        | HostValue::Timestamp(_)
        | HostValue::Date(_)
        | HostValue::Time(_) => false,
        HostValue::Decimal(_) => match value {
            Value::Object(object) => js
                .host_data::<OxDecimal>(object)?
                .is_some_and(|decimal| decimal.inner == *host),
            _ => false,
        },
        HostValue::List(items) => {
            let Value::Object(object) = value else {
                return Ok(false);
            };
            if !js.is_array_object(object)? {
                return Ok(false);
            }
            let length = js.get_property(object, &super::convert::static_key("length")?)?;
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
            if has_special_key(entries) && (entries.contains_key(SET_KEY) || entries.contains_key(WRAP_KEY)) {
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
                if !super::convert::text_is(&name, held_name) || !matches_host(js, engine, &item, held_item, depth + 1)? {
                    return Ok(false);
                }
            }
            expected.next().is_none()
        }
        HostValue::Bytes(_) | HostValue::Object(_) => false,
    })
}

/// [`matches_host`] for a value that may be read with the stack borrowed: `None` where the walk meets anything that
/// could run code (an object that is not ordinary, an accessor, an element read through the prototype), which the
/// caller then compares as [`matches_host`] does, on a copy.
fn matches_in_place<J: Js>(
    js: &mut J,
    engine: &OxideEngine,
    value: &Value,
    host: &HostValue,
    depth: usize,
) -> Result<Option<bool>, Conv> {
    if depth >= MAX_NESTING {
        return Ok(None);
    }
    Ok(Some(match host {
        HostValue::List(items) => {
            let Value::Object(object) = value else {
                return Ok(Some(false));
            };
            // the elements visited where they lie when the array is dense: text and scalars compared there, objects
            // after, walked; anything else as below
            let mut verdict = None;
            let mut lifted = false;
            let mut deferred = Vec::with_capacity(items.len());
            let mut expected = items.iter();
            let visited = js.visit_dense_array(object, &mut |element| {
                let Some(item) = expected.next() else {
                    verdict = Some(false);
                    return false;
                };
                match (element, item) {
                    (DataRef::Text(text), HostValue::String(held)) => {
                        if !text.eq_str(held) {
                            verdict = Some(false);
                            return false;
                        }
                    }
                    (DataRef::Value(element), item) => match scalar_matches(&element, item) {
                        Some(true) => {}
                        Some(false) => {
                            verdict = Some(false);
                            return false;
                        }
                        None => deferred.push((element, item)),
                    },
                    // text against a host value shown as text, or a function: compared as before
                    (DataRef::Text(_) | DataRef::Callable, _) => {
                        lifted = true;
                        return false;
                    }
                }
                true
            })?;
            match visited {
                None => return matches_in_place_slow(js, engine, value, host, depth),
                Some(false) if !lifted => return Ok(Some(verdict.unwrap_or(false))),
                Some(true) => {
                    if expected.next().is_some() {
                        return Ok(Some(false));
                    }
                    for (element, item) in &deferred {
                        match matches_in_place(js, engine, element, item, depth + 1)? {
                            Some(true) => {}
                            other => return Ok(other),
                        }
                    }
                    return Ok(Some(true));
                }
                Some(false) => {}
            }
            let Some(elements) = js.dense_array_values(object)? else {
                return matches_in_place_slow(js, engine, value, host, depth);
            };
            if elements.len() != items.len() {
                return Ok(Some(false));
            }
            for (element, item) in elements.iter().zip(items) {
                match matches_in_place(js, engine, element, item, depth + 1)? {
                    Some(true) => {}
                    other => return Ok(other),
                }
            }
            true
        }
        HostValue::Map(entries) => {
            if has_special_key(entries) && (entries.contains_key(SET_KEY) || entries.contains_key(WRAP_KEY)) {
                return Ok(Some(false));
            }
            let Value::Object(object) = value else {
                return Ok(Some(false));
            };
            // text compared where it lies; the other values after, as they may be objects to walk
            let mut verdict = None;
            let mut lifted = false;
            let mut deferred = Vec::new();
            let mut expected = entries.iter();
            // each key's text compared where it lies too; an object with a symbol key is read as below
            let visited =
                js.visit_plain_own_text(object, Some(&engine.protos.object_prototype), &mut |property| {
                    if !property.enumerable || matches!(property.value, DataRef::Callable) {
                        return true;
                    }
                    let Some((held_name, held_item)) = expected.next() else {
                        verdict = Some(false);
                        return false;
                    };
                    if !property.key.eq_str(held_name) {
                        verdict = Some(false);
                        return false;
                    }
                    match (property.value, held_item) {
                        (DataRef::Text(text), HostValue::String(held)) => {
                            if !text.eq_str(held) {
                                verdict = Some(false);
                                return false;
                            }
                        }
                        // text against a host value shown as text (a time stamp): compared as before
                        (DataRef::Text(_), _) => {
                            lifted = true;
                            return false;
                        }
                        // a number, a flag or null compared here; an object after, walked
                        (DataRef::Value(item), held_item) => match scalar_matches(&item, held_item) {
                            Some(true) => {}
                            Some(false) => {
                                verdict = Some(false);
                                return false;
                            }
                            None => deferred.push((item, held_item)),
                        },
                        (DataRef::Callable, _) => {}
                    }
                    true
                })?;
            match visited {
                None => return matches_in_place_slow(js, engine, value, host, depth),
                Some(false) if !lifted => return Ok(Some(verdict.unwrap_or(false))),
                Some(true) => {
                    if expected.next().is_some() {
                        return Ok(Some(false));
                    }
                    for (item, held_item) in &deferred {
                        match matches_in_place(js, engine, item, held_item, depth + 1)? {
                            Some(true) => {}
                            other => return Ok(other),
                        }
                    }
                    return Ok(Some(true));
                }
                Some(false) => {}
            }
            let Some(properties) = js.plain_own_data(object, Some(&engine.protos.object_prototype))? else {
                return matches_in_place_slow(js, engine, value, host, depth);
            };
            let mut expected = entries.iter();
            for property in properties {
                if !property.enumerable {
                    continue;
                }
                let Some(name) = property.key.to_js_string() else {
                    continue;
                };
                if let Value::Object(inner) = &property.value {
                    if js.as_callable(inner)?.is_some() {
                        continue;
                    }
                }
                let Some((held_name, held_item)) = expected.next() else {
                    return Ok(Some(false));
                };
                if !super::convert::text_is(&name, held_name) {
                    return Ok(Some(false));
                }
                match matches_in_place(js, engine, &property.value, held_item, depth + 1)? {
                    Some(true) => {}
                    other => return Ok(other),
                }
            }
            expected.next().is_none()
        }
        _ => return matches_in_place_slow(js, engine, value, host, depth),
    }))
}

/// [`matches_in_place`] a property at a time: an object [`Js::plain_own_data`] does not read (another prototype, an
/// accessor, a hole).
fn matches_in_place_slow<J: Js>(
    js: &mut J,
    engine: &OxideEngine,
    value: &Value,
    host: &HostValue,
    depth: usize,
) -> Result<Option<bool>, Conv> {
    use quickjs_oxide::engine::api::core::CompleteOrdinaryPropertyDescriptor as Descriptor;
    let own_data = |js: &mut J, object, key: &PropertyKey| -> Result<Option<Value>, Conv> {
        Ok(match js.get_own_property(object, key)? {
            Some(Descriptor::Data { value, .. }) => Some(value),
            _ => None,
        })
    };
    Ok(Some(match host {
        HostValue::List(items) => {
            let Value::Object(object) = value else {
                return Ok(Some(false));
            };
            if !js.is_ordinary_object(object)? {
                return Ok(None);
            }
            if !js.is_array_object(object)? {
                return Ok(Some(false));
            }
            let Some(length) = own_data(js, object, &super::convert::static_key("length")?)? else {
                return Ok(None);
            };
            if !matches!(length, Value::Int(found) if usize::try_from(found).ok() == Some(items.len())) {
                return Ok(Some(false));
            }
            for (index, item) in items.iter().enumerate() {
                let Ok(index) = u32::try_from(index) else {
                    return Ok(None);
                };
                // a hole reads through the prototype, which may run code
                let Some(element) = own_data(js, object, &PropertyKey::index(index))? else {
                    return Ok(None);
                };
                match matches_in_place(js, engine, &element, item, depth + 1)? {
                    Some(true) => {}
                    other => return Ok(other),
                }
            }
            true
        }
        HostValue::Map(entries) => {
            if has_special_key(entries) && (entries.contains_key(SET_KEY) || entries.contains_key(WRAP_KEY)) {
                return Ok(Some(false));
            }
            let Value::Object(object) = value else {
                return Ok(Some(false));
            };
            if !js.is_ordinary_object(object)? {
                return Ok(None);
            }
            if js.get_prototype_of(object)?.as_ref() != Some(&engine.protos.object_prototype) {
                return Ok(Some(false));
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
                let Descriptor::Data { value: item, .. } = descriptor else {
                    return Ok(None);
                };
                if let Value::Object(inner) = &item {
                    if js.as_callable(inner)?.is_some() {
                        continue;
                    }
                }
                let Some((held_name, held_item)) = expected.next() else {
                    return Ok(Some(false));
                };
                if !super::convert::text_is(&name, held_name) {
                    return Ok(Some(false));
                }
                match matches_in_place(js, engine, &item, held_item, depth + 1)? {
                    Some(true) => {}
                    other => return Ok(other),
                }
            }
            expected.next().is_none()
        }
        // the rest reads no property (a decimal's host data at most), so runs no code: as `matches_host`
        _ => matches_host(js, engine, value, host, depth)?,
    }))
}

/// [`matches_host`] of a value that is not an object, which reads nothing of the engine; `None` for an object.
fn scalar_matches(value: &Value, host: &HostValue) -> Option<bool> {
    if matches!(value, Value::Object(_)) {
        return None;
    }
    Some(match host {
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
        HostValue::Timestamp(_) | HostValue::Date(_) | HostValue::Time(_) => text_is(value, &temporal_text(host)),
        HostValue::Decimal(_) | HostValue::List(_) | HostValue::Map(_) | HostValue::Bytes(_) | HostValue::Object(_) => {
            false
        }
    })
}

fn text_is(value: &Value, text: &str) -> bool {
    match value {
        Value::String(found) => super::convert::text_is(found, text),
        _ => false,
    }
}

/// A 64-bit print of a host value, equal for values [`moqui_lib::object::java_equals`] finds equal (a map's entries in
/// any order), made in place with no allocation. Two different values share a print about once in 2^64 checks.
fn fingerprint(value: &HostValue) -> u64 {
    use std::hash::{BuildHasher, Hash, Hasher};
    const SEED: foldhash::quality::FixedState = foldhash::quality::FixedState::with_seed(0x6d71_6f78_6964_6531);
    fn walk(value: &HostValue, hasher: &mut impl Hasher) {
        std::mem::discriminant(value).hash(hasher);
        match value {
            HostValue::Null => {}
            HostValue::String(text) => text.hash(hasher),
            HostValue::Bool(flag) => flag.hash(hasher),
            HostValue::I32(number) => number.hash(hasher),
            HostValue::I64(number) => number.hash(hasher),
            HostValue::F64(number) => number.to_bits().hash(hasher),
            HostValue::Decimal(decimal) => {
                decimal.mantissa().hash(hasher);
                decimal.scale().hash(hasher);
            }
            HostValue::Timestamp(timestamp) => timestamp.hash(hasher),
            HostValue::Date(date) => date.hash(hasher),
            HostValue::Time(time) => time.hash(hasher),
            HostValue::Bytes(bytes) => bytes.hash(hasher),
            HostValue::List(items) => {
                items.len().hash(hasher);
                for item in items {
                    walk(item, hasher);
                }
            }
            HostValue::Map(entries) => {
                // order-free: the entries' own prints summed
                entries.len().hash(hasher);
                let mut sum = 0u64;
                for (key, entry) in entries {
                    let mut inner = SEED.build_hasher();
                    key.hash(&mut inner);
                    walk(entry, &mut inner);
                    sum = sum.wrapping_add(inner.finish());
                }
                sum.hash(hasher);
            }
            HostValue::Object(object) => (std::sync::Arc::as_ptr(&object.0) as *const () as usize).hash(hasher),
        }
    }
    let mut hasher = SEED.build_hasher();
    walk(value, &mut hasher);
    hasher.finish()
}
