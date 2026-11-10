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

//! `EntityFind.iterator()` in a script: Java's `EntityListIterator` over a database cursor, so a long result is never
//! held whole (docs/ui/Screen-Output.md). As in Java it reads forward only unless the find's `resultSetType` is 1004 or
//! 1005 (scroll insensitive or sensitive, which keep the rows read so it can move back and jump); a find for update
//! scrolls. The XML actions' `use-iterator` makes one, and a form-list sent as it renders reads one a row at a time.
//!
//! The iterator holds the thread's transaction scope, so it lives in a thread-local table; the script's object and
//! the context hold an [`IteratorHandle`], and the iterator closes when the last copy of the handle goes, or when it
//! has read its last row. It reads only in the transaction it opened in; a nested transaction (a `force-new` service
//! for each row) runs on another connection while it waits. While it is open its connection takes no other cursor. On
//! MySQL, whose open result takes no other query, the rows are read into memory instead, so a lookup for each row still
//! works.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::ThreadId;

use moqui_lib::error::Error;
use moqui_lib::object::{HostObject, Value};

use super::entities::{lib, list_value, value_of};
use super::{method, Call, HostClass, HostError, Ret};
use crate::entity::{EntityFind, EntityListIterator, EntityValue};

thread_local! {
    /// The open iterators of this thread, by handle id.
    static OPEN: RefCell<HashMap<u64, EntityListIterator>> = RefCell::new(HashMap::new());
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// An iterator as a host value: the id of its iterator in its thread's table.
pub(crate) struct IteratorHandle {
    id: u64,
    thread: ThreadId,
    entity: String,
}

impl IteratorHandle {
    /// The next row; `None` after the last, when the cursor has closed itself.
    ///
    /// # Errors
    ///
    /// Returns an error when the read fails, the iterator is closed, or its transaction has ended.
    pub(crate) fn next(&self) -> Result<Option<EntityValue>, Error> {
        self.with(EntityListIterator::next_value)
    }

    /// Whether a row follows, read ahead.
    ///
    /// # Errors
    ///
    /// As [`IteratorHandle::next`].
    pub(crate) fn has_next(&self) -> Result<bool, Error> {
        self.with(EntityListIterator::has_next)
    }

    /// Every row not read yet, which closes the cursor.
    ///
    /// # Errors
    ///
    /// As [`IteratorHandle::next`].
    pub(crate) fn rest(&self) -> Result<Vec<EntityValue>, Error> {
        let mut rows = Vec::new();
        while let Some(row) = self.next()? {
            rows.push(row);
        }
        Ok(rows)
    }

    /// Close the iterator now; a later read is refused.
    pub(crate) fn close(&self) {
        let taken = OPEN.with(|open| open.borrow_mut().remove(&self.id));
        drop(taken);
    }

    /// `body` with the iterator, taken out of the table while it runs, so a read that runs a nested script (an entity
    /// ECA rule, a lookup that opens another iterator) can use the table.
    ///
    /// # Errors
    ///
    /// Returns `body`'s error, or an error when the iterator is closed or this is not the thread that opened it.
    pub(crate) fn with<T>(&self, body: impl FnOnce(&mut EntityListIterator) -> Result<T, Error>) -> Result<T, Error> {
        if std::thread::current().id() != self.thread {
            return Err(Error::new(format!(
                "the iterator of {} is read on the thread that opened it",
                self.entity
            )));
        }
        let taken = OPEN.with(|open| open.borrow_mut().remove(&self.id));
        let Some(mut iterator) = taken else {
            return Err(Error::new(format!("the iterator of {} is closed", self.entity)));
        };
        let result = body(&mut iterator);
        OPEN.with(|open| open.borrow_mut().insert(self.id, iterator));
        result
    }
}

impl Drop for IteratorHandle {
    fn drop(&mut self) {
        if std::thread::current().id() != self.thread {
            // only its own thread reaches its table; that one closes it when the thread ends
            return;
        }
        let taken = OPEN
            .try_with(|open| open.try_borrow_mut().ok().and_then(|mut open| open.remove(&self.id)))
            .ok()
            .flatten();
        // closed here, outside the table's borrow
        drop(taken);
    }
}

/// Open an iterator over `find`, as a handle. On MySQL the rows are read into memory ([module docs](self)).
///
/// # Errors
///
/// Returns an error when the find fails.
pub(crate) fn open(find: EntityFind) -> Result<IteratorHandle, Error> {
    let entity = find.entity_name().to_string();
    let iterator = if find.facade().streams_beside_queries(&entity) {
        find.iterator()?
    } else {
        EntityListIterator::of_list(find.facade().clone(), entity.clone(), find.list()?)
    };
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    OPEN.with(|open| open.borrow_mut().insert(id, iterator));
    Ok(IteratorHandle {
        id,
        thread: std::thread::current().id(),
        entity,
    })
}

/// The iterator a value holds, if it is one.
pub(crate) fn iterator_of(value: &Value) -> Option<Arc<IteratorHandle>> {
    match value {
        Value::Object(object) => object.downcast::<IteratorHandle>(),
        _ => None,
    }
}

/// `for (const row of iterator)` over `next()`, and Java's `writeXmlText(writer, prefix, dependentLevels)` and
/// `writeXmlTextMaster(writer, prefix, masterName)`, which write to any object with a `write` method.
const ITERATOR_PROTOTYPE_JS: &str = r#"(function (proto, hidden) {
    const define = (name, value) => Object.defineProperty(proto, name, { value, writable: true, configurable: true });
    define(Symbol.iterator, function* () {
        let row;
        while ((row = this.next()) != null) yield row;
    });
    const written = (writer, made) => {
        if (writer == null || typeof writer.write !== "function") {
            throw new TypeError("a writer with a write method is needed");
        }
        writer.write(made[0]);
        return made[1];
    };
    const text = (value) => value == null ? "" : String(value);
    define("writeXmlText", function writeXmlText(writer, prefix, dependentLevels) {
        return written(writer, hidden.xmlText.call(this, text(prefix), Number(dependentLevels || 0), ""));
    });
    define("writeXmlTextMaster", function writeXmlTextMaster(writer, prefix, masterName) {
        return written(writer, hidden.xmlText.call(this, text(prefix), 0, masterName == null ? "default" : text(masterName)));
    });
})"#;

/// An iterator's methods, as Java's `EntityListIterator`.
pub(crate) static ITERATOR: HostClass = HostClass {
    name: "host:EntityListIterator",
    methods: &[
        method!("EntityListIterator", "next", [], iterator_next),
        method!("EntityListIterator", "hasNext", [], iterator_has_next),
        method!("EntityListIterator", "previous", [], iterator_previous),
        method!("EntityListIterator", "hasPrevious", [], iterator_has_previous),
        method!("EntityListIterator", "nextIndex", [], iterator_next_index),
        method!("EntityListIterator", "previousIndex", [], iterator_previous_index),
        method!("EntityListIterator", "first", [], iterator_first),
        method!("EntityListIterator", "last", [], iterator_last),
        method!("EntityListIterator", "beforeFirst", [], iterator_before_first),
        method!("EntityListIterator", "afterLast", [], iterator_after_last),
        method!("EntityListIterator", "absolute", [Int], iterator_absolute),
        method!("EntityListIterator", "relative", [Int], iterator_relative),
        method!("EntityListIterator", "currentIndex", [], iterator_current_index),
        method!("EntityListIterator", "currentEntityValue", [], iterator_current_value),
        method!("EntityListIterator", "setFetchSize", [Int], iterator_set_fetch_size),
        method!("EntityListIterator", "getCompleteList", [OptBool], iterator_complete),
        method!(
            "EntityListIterator",
            "getPartialList",
            [Int, Int, OptBool],
            iterator_partial
        ),
        method!("EntityListIterator", "close", [], None, leaf, iterator_close),
    ],
    getters: &[],
    hidden: &[method!(
        "EntityListIterator",
        "xmlText",
        [Text, Int, Text],
        iterator_xml_text
    )],
    shim_js: Some(ITERATOR_PROTOTYPE_JS),
    shim_members: &["writeXmlText", "writeXmlTextMaster"],
};

/// `EntityFind.iterator()`: the find's rows through a cursor. The find stays the builder's.
pub(super) fn find_iterator(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let find = super::entities::peek_find_of(call)?;
    let handle = open(find).map_err(lib)?;
    Ok(Ret::Object(HostObject::new(handle), &ITERATOR))
}

/// `body` with the receiver's iterator.
fn on_this<T>(call: &Call<'_>, body: impl FnOnce(&mut EntityListIterator) -> Result<T, Error>) -> Result<T, HostError> {
    call.this::<IteratorHandle>()?.with(body).map_err(lib)
}

fn row(row: Option<EntityValue>) -> Ret {
    Ret::Value(row.map_or(Value::Null, value_of))
}

fn iterator_next(call: &mut Call<'_>) -> Result<Ret, HostError> {
    on_this(call, EntityListIterator::next_value).map(row)
}

fn iterator_has_next(call: &mut Call<'_>) -> Result<Ret, HostError> {
    on_this(call, EntityListIterator::has_next).map(Ret::from)
}

fn iterator_previous(call: &mut Call<'_>) -> Result<Ret, HostError> {
    on_this(call, EntityListIterator::previous_value).map(row)
}

fn iterator_has_previous(call: &mut Call<'_>) -> Result<Ret, HostError> {
    on_this(call, |iterator| Ok(iterator.has_previous())).map(Ret::from)
}

/// Java's `nextIndex`: `currentIndex() + 1`, whether or not a row is there.
fn iterator_next_index(call: &mut Call<'_>) -> Result<Ret, HostError> {
    on_this(call, |iterator| Ok(iterator.current_index() + 1)).map(index)
}

/// Java's `previousIndex`: `currentIndex() - 1`.
fn iterator_previous_index(call: &mut Call<'_>) -> Result<Ret, HostError> {
    on_this(call, |iterator| Ok(iterator.current_index() - 1)).map(index)
}

fn iterator_first(call: &mut Call<'_>) -> Result<Ret, HostError> {
    on_this(call, EntityListIterator::first).map(Ret::from)
}

fn iterator_last(call: &mut Call<'_>) -> Result<Ret, HostError> {
    on_this(call, EntityListIterator::last).map(Ret::from)
}

fn iterator_before_first(call: &mut Call<'_>) -> Result<Ret, HostError> {
    on_this(call, EntityListIterator::before_first).map(|()| Ret::Undefined)
}

fn iterator_after_last(call: &mut Call<'_>) -> Result<Ret, HostError> {
    on_this(call, EntityListIterator::after_last).map(|()| Ret::Undefined)
}

fn iterator_absolute(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let row = i64::from(call.int(0).unwrap_or(0));
    on_this(call, |iterator| iterator.absolute(row)).map(Ret::from)
}

fn iterator_relative(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let rows = i64::from(call.int(0).unwrap_or(0));
    on_this(call, |iterator| iterator.relative(rows)).map(Ret::from)
}

fn iterator_current_index(call: &mut Call<'_>) -> Result<Ret, HostError> {
    on_this(call, |iterator| Ok(iterator.current_index())).map(index)
}

fn iterator_current_value(call: &mut Call<'_>) -> Result<Ret, HostError> {
    on_this(call, EntityListIterator::current_value).map(row)
}

/// Java's `setFetchSize`: a hint; the driver fetches in batches of its own.
fn iterator_set_fetch_size(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let rows = usize::try_from(call.int(0).unwrap_or(0).max(0)).unwrap_or(0);
    on_this(call, |iterator| {
        iterator.set_fetch_size(rows);
        Ok(())
    })
    .map(|()| Ret::Undefined)
}

/// Java's `getCompleteList(closeAfter)`: every row as a list, from the first when the cursor can go back, else from
/// here on.
fn iterator_complete(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let close_after = call.bool(0).unwrap_or(false);
    let list = on_this(call, EntityListIterator::get_complete_list);
    if close_after {
        call.this::<IteratorHandle>()?.close();
    }
    Ok(Ret::Value(list_value(list?.into_values())))
}

/// Java's `getPartialList(offset, limit, closeAfter)`: up to `limit` rows from row `offset`, 1-based from the first
/// (0 is 1), so a forward-only cursor gives a part only ahead of where it is.
fn iterator_partial(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let offset = i64::from(call.int(0).unwrap_or(0));
    let limit = usize::try_from(call.int(1).unwrap_or(0).max(0)).unwrap_or(0);
    let close_after = call.bool(2).unwrap_or(false);
    let list = on_this(call, |iterator| iterator.get_partial_list(offset, limit));
    if close_after {
        call.this::<IteratorHandle>()?.close();
    }
    Ok(Ret::Value(list_value(list?.into_values())))
}

fn iterator_close(call: &mut Call<'_>) -> Result<Ret, HostError> {
    call.this::<IteratorHandle>()?.close();
    Ok(Ret::Undefined)
}

/// The hidden `xmlText(prefix, dependentLevels, masterName)` of the shim's `writeXmlText`: `[text, records]`; an empty
/// master name writes the dependents to `dependentLevels`.
fn iterator_xml_text(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let prefix = call.text_or_empty(0);
    let levels = i64::from(call.int(1).unwrap_or(0));
    let master = call.text(2).filter(|name| !name.is_empty()).map(str::to_string);
    let (text, records) = on_this(call, |iterator| {
        iterator.write_xml_text(&prefix, levels, master.as_deref())
    })?;
    Ok(Ret::Value(Value::List(vec![
        Value::String(text),
        Value::I64(i64::try_from(records).unwrap_or(i64::MAX)),
    ])))
}

fn index(value: i64) -> Ret {
    Ret::Value(Value::I64(value))
}
